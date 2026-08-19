//! Refresh lifted-lambda / AllocClosure Fun return types after mono.

use crate::find_top_level_local_def;
use crate::ir::{Block, CoreModule, Value};
use crate::visit::collect_closure_cap_funrefs;
use lumia_hir::Sym;
use lumia_ty::Type;
use rustc_hash::FxHashMap as HashMap;

/// Upgrade `__lam_*` return types from callee tables / float locals after mono.
pub(super) fn refresh_lifted_lambda_rets(module: &mut CoreModule) {
    let (mut fun_ret_tys, fun_param_tys) = crate::ModuleTables::from_module(module).into_maps();
    let hof = super::super::float_abi::HofSets::from_module_funs(
        module
            .functions
            .iter()
            .map(|f| (&f.name, f.params.as_slice(), &f.body)),
    );
    // lam → (capture_index → callee fun name) from AllocClosure sites.
    let mut cap_funs: HashMap<Sym, HashMap<u32, Sym>> = HashMap::default();
    let mut lam_caps: HashMap<Sym, Vec<crate::Local>> = HashMap::default();
    for fun in &module.functions {
        let mut funref_locals: HashMap<u32, Sym> = HashMap::default();
        collect_closure_cap_funrefs(&fun.body, &mut funref_locals, &mut cap_funs);
        crate::visit::collect_alloc_closure_caps(&fun.body, &mut lam_caps);
    }
    let by_local = module.channel_elem_by_local.clone();
    let module_hint = module.channel_elem_hint.clone();
    // Call/heap lookup uses `fun_ret_tys`; keep it in sync and iterate so
    // `spawn { go() }` sees `go`'s Float after `go` itself is refreshed.
    let mut changed = true;
    let mut guard = 0u32;
    while changed && guard < 64 {
        changed = false;
        guard += 1;
        // Rebuild after each round so capture types see upgraded list/ADT/task rets.
        let fun_cap_tys =
            super::super::float_abi::collect_fun_cap_tys(module, &fun_ret_tys, &fun_param_tys);
        let empty_caps = HashMap::default();
        for fun in &mut module.functions {
            if !fun.is_lifted_lambda() {
                continue;
            }
            // Float/Bool/Unit are final. String/Char may still upgrade to Float
            // (`Err("e") alt 9.5`: then=AdtField(String), else=Float).
            // Fun rets still refine (e.g. Fun([Int],List(Int)) → Fun([Float],Float)).
            if matches!(fun.ret_ty, Type::Float | Type::Bool | Type::Unit) {
                continue;
            }
            let this_caps = fun_cap_tys.get(fun.name.as_str()).unwrap_or(&empty_caps);
            let mut new_ty: Option<Type> = None;
            if super::super::float_abi::block_result_is_float(&fun.body, &fun_ret_tys) {
                new_ty = Some(Type::Float);
            } else if super::super::float_abi::block_result_is_bool(&fun.body) {
                new_ty = Some(Type::Bool);
            } else if super::super::float_abi::block_result_is_unit(&fun.body) {
                new_ty = Some(Type::Unit);
            } else {
                let caps = lam_caps.get(fun.name.as_str()).map(|c| c.as_slice());
                if let Some(t) = super::super::float_abi::block_result_channel_ty(
                    &fun.body,
                    &by_local,
                    module_hint.as_ref(),
                    caps,
                ) {
                    new_ty = Some(t);
                } else if let Some(t) = super::super::float_abi::block_result_channel_recv_ty(
                    &fun.body,
                    &by_local,
                    module_hint.as_ref(),
                    caps,
                ) {
                    new_ty = Some(t);
                } else if let Some(t) = super::super::float_abi::block_result_heap_ty_caps(
                    &fun.body,
                    &fun_ret_tys,
                    &fun_param_tys,
                    this_caps,
                ) {
                    match &t {
                        Type::Float
                        | Type::Bool
                        | Type::Unit
                        | Type::Fun(_, _, _)
                        | Type::String
                        | Type::Char => {
                            new_ty = Some(t);
                        }
                        Type::List(_)
                        | Type::Map(_, _)
                        | Type::Set(_)
                        | Type::Adt { .. }
                        | Type::Task(_)
                        | Type::Channel(_)
                            if matches!(
                                &fun.ret_ty,
                                Type::Int
                                    | Type::Var(_)
                                    | Type::List(_)
                                    | Type::Map(_, _)
                                    | Type::Set(_)
                                    | Type::Adt { .. }
                                    | Type::Task(_)
                                    | Type::Channel(_)
                                    | Type::Fun(_, _, _)
                            ) =>
                        {
                            new_ty = Some(t);
                        }
                        _ => {}
                    }
                }
                if new_ty.is_none() {
                    let caps = cap_funs.get(fun.name.as_str());
                    let from_call =
                        super::super::float_abi::block_result_callee_ty(&fun.body, &fun_ret_tys);
                    let from_apply = super::super::float_abi::block_result_known_hof_ty(
                        &fun.body,
                        &hof,
                        &fun_ret_tys,
                        caps,
                    );
                    let from_icall = caps.and_then(|c| {
                        super::super::float_abi::block_result_icall_cap_ty_by_index(
                            &fun.body,
                            c,
                            &fun_ret_tys,
                        )
                    });
                    let from_fun = super::super::float_abi::block_result_fun_ty(
                        &fun.body,
                        &fun_ret_tys,
                        &fun_param_tys,
                    );
                    if let Some(t) = from_call.or(from_apply).or(from_icall).or(from_fun) {
                        match &t {
                            Type::Float | Type::Fun(_, _, _) | Type::String | Type::Char => {
                                new_ty = Some(t)
                            }
                            Type::List(_)
                            | Type::Map(_, _)
                            | Type::Set(_)
                            | Type::Adt { .. }
                            | Type::Task(_)
                                if matches!(
                                    &fun.ret_ty,
                                    Type::Int
                                        | Type::Var(_)
                                        | Type::List(_)
                                        | Type::Map(_, _)
                                        | Type::Set(_)
                                        | Type::Adt { .. }
                                        | Type::Task(_)
                                        | Type::Fun(_, _, _)
                                ) =>
                            {
                                new_ty = Some(t);
                            }
                            _ => {}
                        }
                    }
                }
            }
            if let Some(t) = new_ty {
                // Join with existing ret so List(Fun([Int],Int)) can become
                // List(Fun([Float],Float)) once callee tables are Float.
                let merged =
                    super::super::float_abi::prefer_concrete_heap_ty(fun.ret_ty.clone(), t);
                if merged != fun.ret_ty {
                    fun.ret_ty = merged.clone();
                    fun_ret_tys.insert(fun.name.clone(), merged);
                    changed = true;
                }
            }
        }
        // Propagate AllocClosure body rets onto curried HOF Fun returns.
        if refresh_alloc_closure_fun_rets_round(module, &mut fun_ret_tys) {
            changed = true;
        }
    }
}

pub(super) fn refresh_alloc_closure_fun_rets(module: &mut CoreModule) {
    let (mut fun_ret_tys, _) = crate::ModuleTables::from_module(module).into_maps();
    let _ = refresh_alloc_closure_fun_rets_round(module, &mut fun_ret_tys);
}

/// Upgrade `ret = AllocClosure(lam)` to `Fun` from `lam`'s current signature.
/// Returns whether any function ret changed.
pub(super) fn refresh_alloc_closure_fun_rets_round(
    module: &mut CoreModule,
    fun_ret_tys: &mut HashMap<Sym, Type>,
) -> bool {
    let lam_sig: HashMap<Sym, (Vec<Type>, Type)> = module
        .functions
        .iter()
        .filter(|f| f.is_lifted_lambda())
        .map(|f| {
            // Drop env param for the user-facing Fun type.
            let params = if f.params.len() > 1 {
                f.param_tys[1..].to_vec()
            } else {
                Vec::new()
            };
            (f.name.clone(), (params, f.ret_ty.clone()))
        })
        .collect();

    let mut changed = false;
    for fun in &mut module.functions {
        if let Some(lam) = result_alloc_closure_fun(&fun.body) {
            if let Some((params, ret)) = lam_sig.get(&lam) {
                // Concrete body ret / float params — or already-Fun ret that can refine.
                let interesting = matches!(
                    ret,
                    Type::Float
                        | Type::Bool
                        | Type::Fun(_, _, _)
                        | Type::Unit
                        | Type::String
                        | Type::Char
                ) || params.iter().any(|t| {
                    matches!(
                        t,
                        Type::Float | Type::Fun(_, _, _) | Type::Bool | Type::String | Type::Char
                    )
                }) || matches!(fun.ret_ty, Type::Fun(_, _, _));
                if !interesting {
                    continue;
                }
                let candidate = Type::Fun(params.clone(), Box::new(ret.clone()), fun.effect);
                let merged =
                    super::super::float_abi::prefer_concrete_heap_ty(fun.ret_ty.clone(), candidate);
                if merged != fun.ret_ty {
                    fun.ret_ty = merged.clone();
                    fun_ret_tys.insert(fun.name.clone(), merged);
                    changed = true;
                }
            }
        }
    }
    changed
}

pub(super) fn result_alloc_closure_fun(block: &Block) -> Option<Sym> {
    let r = block.result?;
    match find_top_level_local_def(block, r.0)? {
        Value::AllocClosure { fun, .. } => Some(fun.name.clone()),
        Value::Local(src) => match find_top_level_local_def(block, src.0)? {
            Value::AllocClosure { fun, .. } => Some(fun.name.clone()),
            _ => None,
        },
        _ => None,
    }
}
