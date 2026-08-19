use super::super::fun_index::FunIndex;
use super::super::key::ground_open_vars;
use super::super::ret_ty::{block_result_fixed_ty, param_ty_map, refine_mono_container_ret};
use crate::ir::{CoreFun, CoreModule};
use lumia_syntax::Sym;
use lumia_ty::Type;
use rustc_hash::FxHashMap as HashMap;

/// When a mono clone has Float / List[Float] / … formals but the generic still
/// carries ABI `Int` / `List[Int]`, copy the clone's ground types onto the
/// generic. Missed call-site rewrites then still get correct float arith in
/// codegen (instead of `smul` on IEEE bits → `lumia_trap_overflow`).
pub(super) fn upgrade_generic_param_tys_from_clones(module: &mut CoreModule) {
    let upgrades: Vec<(Sym, Vec<Type>, Type)> = {
        let mut best: HashMap<Sym, (Vec<Type>, Type)> = HashMap::default();
        for f in &module.functions {
            let Some(orig) = f.mono_of.as_ref() else {
                continue;
            };
            let entry = best
                .entry(orig.clone())
                .or_insert_with(|| (f.param_tys.clone(), f.ret_ty.clone()));
            for (i, ty) in f.param_tys.iter().enumerate() {
                if i >= entry.0.len() {
                    entry.0.resize(i + 1, Type::Int);
                }
                if mono_ty_more_precise(ty, &entry.0[i]) {
                    entry.0[i] = ty.clone();
                }
            }
            if mono_ty_more_precise(&f.ret_ty, &entry.1) {
                entry.1 = f.ret_ty.clone();
            }
        }
        best.into_iter()
            .map(|(name, (ps, ret))| (name, ps, ret))
            .collect()
    };
    for fun in &mut module.functions {
        // Scheme-poly generics keep erased Int/Var ABI on purpose: Int call
        // sites share the generic while Float/Bool sites use `$Float` / `$Bool`
        // clones. Copying clone ground types onto the generic makes `dbl(1)` /
        // `id(1)` use Float/Bool println (or float arith) on Int bits.
        if fun.mono_of.is_some() || fun.external.is_some() || fun.scheme_poly {
            continue;
        }
        let Some((_, ps, ret)) = upgrades.iter().find(|(n, _, _)| n == &fun.name) else {
            continue;
        };
        for (i, ty) in ps.iter().enumerate() {
            if i >= fun.param_tys.len() {
                fun.param_tys.resize(i + 1, Type::Int);
            }
            if mono_ty_more_precise(ty, &fun.param_tys[i]) {
                fun.param_tys[i] = ty.clone();
            }
        }
        if mono_ty_more_precise(ret, &fun.ret_ty) {
            fun.ret_ty = ret.clone();
        }
    }
}

fn mono_ty_more_precise(new: &Type, old: &Type) -> bool {
    match (new, old) {
        (Type::Float, Type::Int | Type::Var(_)) => true,
        (Type::Bool | Type::String | Type::Char, Type::Int | Type::Var(_)) => true,
        (Type::List(n), Type::List(o)) => {
            mono_ty_more_precise(n, o) || matches!(o.as_ref(), Type::Int | Type::Var(_))
        }
        (Type::List(_), Type::Int | Type::Var(_)) => true,
        (Type::Set(n), Type::Set(o)) => {
            mono_ty_more_precise(n, o) || matches!(o.as_ref(), Type::Int | Type::Var(_))
        }
        (Type::Set(_), Type::Int | Type::Var(_)) => true,
        (Type::Map(nk, nv), Type::Map(ok, ov)) => {
            mono_ty_more_precise(nk, ok) || mono_ty_more_precise(nv, ov)
        }
        (Type::Map(_, _), Type::Int | Type::Var(_)) => true,
        (
            Type::Adt {
                name: n,
                params: np,
            },
            Type::Adt {
                name: o,
                params: op,
            },
        ) if n == o => np
            .iter()
            .zip(op.iter())
            .any(|(a, b)| mono_ty_more_precise(a, b)),
        (Type::Adt { .. }, Type::Int | Type::Var(_)) => true,
        _ => false,
    }
}

pub(super) fn refresh_erased_mono_return_types(module: &mut CoreModule) {
    // Analyze immutably first so we need not clone the whole function table.
    let upgrades: Vec<(usize, Type)> = {
        let index = FunIndex::new(
            &module.functions,
            &module.sum_max_arity,
            &module.trait_methods,
            module.channel_elem_hint.as_ref(),
        );
        let traits = &module.trait_methods;
        module
            .functions
            .iter()
            .enumerate()
            .filter_map(|(i, fun)| {
                let params = param_ty_map(fun);
                let t = block_result_fixed_ty(&fun.body, &index, traits, &params)?;
                let upgrade = matches!(
                    (&fun.ret_ty, &t),
                    (
                        Type::Int | Type::Var(_),
                        Type::Float
                            | Type::Bool
                            | Type::String
                            | Type::Char
                            | Type::Adt { .. }
                            | Type::List(_)
                            | Type::Map(_, _)
                            | Type::Set(_),
                    )
                );
                upgrade.then_some((i, t))
            })
            .collect()
    };
    for (i, t) in upgrades {
        module.functions[i].ret_ty = t;
    }
}

/// Ret type for a mono clone: prefer body structure + formals; Num poly
/// (`{ x -> x + x }`) falls back to MonoKey when the body has no fixed ret.
pub(super) fn mono_clone_ret_ty(fun: &CoreFun, inferred: &Type, index: &FunIndex<'_>) -> Type {
    mono_clone_ret_ty_parts(
        &fun.body,
        &fun.params,
        &fun.param_tys,
        &fun.ret_ty,
        inferred,
        index,
    )
}

/// Like [`mono_clone_ret_ty`] but without requiring a full [`CoreFun`] (avoids
/// cloning the body just to probe ret when the clone may be discarded).
pub(super) fn mono_clone_ret_ty_parts(
    body: &crate::ir::Block,
    params: &[crate::ir::Local],
    param_tys: &[Type],
    fun_ret_ty: &Type,
    inferred: &Type,
    index: &FunIndex<'_>,
) -> Type {
    let mut param_map = HashMap::default();
    for (p, t) in params.iter().zip(param_tys.iter()) {
        param_map.insert(p.0, t.clone());
    }
    let raw = if let Some(t) = block_result_fixed_ty(body, index, index.trait_methods, &param_map) {
        // Nested `andThen` bodies often join to `Option[Option[Int]]` / `Option[Int]`
        // while the FunRef key already knows `Option[Float]` — prefer the key.
        merge_mono_ret_with_inferred(t, inferred)
    } else {
        match fun_ret_ty {
            Type::String => Type::String,
            Type::Bool => Type::Bool,
            Type::List(e) if matches!(e.as_ref(), Type::Int) => inferred.clone(),
            Type::Var(_) => inferred.clone(),
            Type::Int | Type::Float | Type::Char | Type::Unit => match inferred {
                Type::Adt { .. }
                | Type::List(_)
                | Type::Map(_, _)
                | Type::Set(_)
                | Type::Task(_)
                | Type::Channel(_)
                | Type::String
                | Type::Bool => fun_ret_ty.clone(),
                _ => inferred.clone(),
            },
            Type::Adt { .. }
            | Type::List(_)
            | Type::Map(_, _)
            | Type::Set(_)
            | Type::Task(_)
            | Type::Channel(_)
            | Type::Tuple(_)
            | Type::TuplePrefix(_) => refine_mono_container_ret(fun_ret_ty, inferred),
            _ => inferred.clone(),
        }
    };
    // Open Vars survive refine when Err/elem slots stay polymorphic — then
    // `type_to_mono` fails and follow-on `unwrapOr` never clones.
    ground_open_vars(raw)
}

/// When body typing lags behind the mono key (erased Int / nested Option), take
/// the inferred payload — fixes `andThen(…, { x -> andThen(…) })` then unwrapOr.
pub(super) fn merge_mono_ret_with_inferred(body: Type, inferred: &Type) -> Type {
    match (&body, inferred) {
        (
            Type::Adt {
                name: bn,
                params: bp,
            },
            Type::Adt {
                name: inan,
                params: ip,
            },
        ) if bn == inan && lumia_hir::is_option_or_result(bn) => {
            let body_payload = bp.first();
            let inf_payload = ip.first();
            if option_result_payload_weaker(body_payload, inf_payload) {
                return inferred.clone();
            }
            refine_mono_container_ret(&body, inferred)
        }
        (Type::Int | Type::Var(_), _) => match inferred {
            // Soft/`Var` body may still need scalar upgrades from the MonoKey
            // (Float ABI, bool, …). **Concrete Int must not** — otherwise
            // `{ x -> 1 }` specialized at Float (`__lam$Float`) gets `ret=Float`
            // and auto-parallel map tags Int `1` as IEEE (denormal Show).
            Type::Float | Type::Bool | Type::String | Type::Char | Type::Fun(_, _, _)
                if matches!(body, Type::Var(_)) =>
            {
                inferred.clone()
            }
            // Do **not** promote body `Int` to List/Map/ADT from the MonoKey.
            // `{ xs -> xs.len() }` body is Int while the key is `$List_Int`;
            // preferring List made Call results look heap-ish (retain on `3`).
            Type::Adt { .. }
            | Type::List(_)
            | Type::Map(_, _)
            | Type::Set(_)
            | Type::Task(_)
            | Type::Channel(_)
                if matches!(body, Type::Var(_)) =>
            {
                inferred.clone()
            }
            _ => body,
        },
        (
            Type::Adt { .. }
            | Type::List(_)
            | Type::Map(_, _)
            | Type::Set(_)
            | Type::Task(_)
            | Type::Channel(_),
            _,
        ) => refine_mono_container_ret(&body, inferred),
        _ => body,
    }
}

fn option_result_payload_weaker(body: Option<&Type>, inferred: Option<&Type>) -> bool {
    let Some(inf) = inferred else {
        return false;
    };
    // Inferred must be a concrete payload worth preferring.
    match inf {
        Type::Int | Type::Var(_) => return false,
        Type::List(e) if matches!(e.as_ref(), Type::Int | Type::Var(_)) => return false,
        _ => {}
    }
    match body {
        None => true,
        // Scalar body from `AdtField(Some(inner))` is concrete. Do not prefer a
        // nested `Option`/`Result` MonoKey shape (`flatten(Some(Some(3)))`
        // inferred `Option[Option[Int]]` over body `Option[Int]`).
        Some(Type::Int | Type::Var(_)) => matches!(
            inf,
            Type::Float | Type::Bool | Type::String | Type::Char | Type::Fun(_, _, _)
        ),
        Some(Type::List(e)) if matches!(e.as_ref(), Type::Int | Type::Var(_)) => {
            matches!(
                inf,
                Type::Float
                    | Type::Bool
                    | Type::String
                    | Type::Char
                    | Type::Fun(_, _, _)
                    | Type::List(_)
            )
        }
        // `Option[Option[Int]]` vs `Option[Float]` from nested andThen join.
        Some(Type::Adt { name, params }) if lumia_hir::is_option_or_result(name) => {
            params
                .first()
                .is_none_or(|p| matches!(p, Type::Int | Type::Var(_)))
                || !matches!(inf, Type::Adt { name: n, .. } if lumia_hir::is_option_or_result(n))
        }
        _ => false,
    }
}

/// Call-site ret while scanning/rewriting: same body-first strategy as
/// [`mono_clone_ret_ty`]. With call-site formals, `touch(b, eps)` resolves to
/// the product (not MonoKey's trailing `Float`), so later `addx` keys match.
pub(super) fn call_site_mono_ret(
    fun: &CoreFun,
    inferred: &Type,
    call_param_tys: &[Type],
    index: &FunIndex<'_>,
) -> Type {
    let mut param_map: HashMap<u32, Type> = HashMap::default();
    for (i, p) in fun.params.iter().enumerate() {
        let ty = call_param_tys
            .get(i)
            .cloned()
            .or_else(|| fun.param_tys.get(i).cloned())
            .unwrap_or(Type::Int);
        param_map.insert(p.0, ty);
    }
    let raw =
        if let Some(t) = block_result_fixed_ty(&fun.body, index, index.trait_methods, &param_map) {
            merge_mono_ret_with_inferred(t, inferred)
        } else {
            match &fun.ret_ty {
                Type::String => Type::String,
                Type::Bool => Type::Bool,
                Type::List(e) if matches!(e.as_ref(), Type::Int) => inferred.clone(),
                Type::Var(_) => inferred.clone(),
                Type::Int | Type::Float | Type::Char | Type::Unit => match inferred {
                    Type::Adt { .. }
                    | Type::List(_)
                    | Type::Map(_, _)
                    | Type::Set(_)
                    | Type::Task(_)
                    | Type::Channel(_)
                    | Type::String
                    | Type::Bool => fun.ret_ty.clone(),
                    _ => inferred.clone(),
                },
                Type::Adt { .. }
                | Type::List(_)
                | Type::Map(_, _)
                | Type::Set(_)
                | Type::Task(_)
                | Type::Channel(_)
                | Type::Tuple(_)
                | Type::TuplePrefix(_) => refine_mono_container_ret(&fun.ret_ty, inferred),
                _ => inferred.clone(),
            }
        };
    ground_open_vars(raw)
}
