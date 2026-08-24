use super::fun_index::FunIndex;
use super::key::{args_mono_key, materialize_mono_param_tys, types_mono_key, MonoKey, MonoKind};
use super::ret_ty::{block_result_fixed_ty, param_ty_map, refine_mono_container_ret};
use super::traits::directize_block;
use crate::ir::{Block, CoreFun, CoreModule, Local, Op, Value};
use crate::value_ty::{infer_value_ty_ctx, InferValueCtx};
use lumi_hir::Builtin;
use lumi_ty::Type;
use rustc_hash::{FxHashMap as HashMap, FxHashSet};

/// Max clone-discovery iterations.
///
/// Transitive FunRef HOF chains (`optMap` → `apply` → `dbl`) typically need 2–3
/// rounds. This cap is a safety fuse against non-termination bugs; the loop
/// converges early when a round adds no clones.
const MAX_MONO_CLONE_ROUNDS: usize = 8;

/// Scheme-driven monomorphization:
/// 1. **Collect clones** until fixed point (scan → clone worklist).
/// 2. **Rewrite** call sites to mangled clones (single pass).
/// 3. **Refresh** erased HOF return types from final bodies (single pass).
pub(crate) fn specialize_mono_calls(module: &mut CoreModule) {
    let renames = collect_mono_clones_until_fixed_point(module);
    if renames.is_empty() {
        return;
    }
    rewrite_all_mono_call_sites(module, &renames);
    // After all clones exist, upgrade erased Int rets on HOF wrappers whose
    // bodies now `Call(dbl$Float, …)` (directize order within a round varies).
    refresh_erased_mono_return_types(module);
    // Toehold: thin FunRef wrappers that only forward to a concrete Call share
    // that target at call sites (avoid an extra frame / duplicate body emit).
    elide_trivial_mono_forwarders(module);
}

/// Fixed-point: scan all bodies for needed `(generic, MonoKey)` clones, append
/// them, repeat until the worklist is empty or [`MAX_MONO_CLONE_ROUNDS`] hits.
fn collect_mono_clones_until_fixed_point(
    module: &mut CoreModule,
) -> HashMap<(String, MonoKey), String> {
    let mut renames: HashMap<(String, MonoKey), String> = HashMap::default();
    for _round in 0..MAX_MONO_CLONE_ROUNDS {
        if !specialize_mono_round(module, &mut renames) {
            break;
        }
    }
    renames
}

/// Rewrite every direct `Call(generic, …)` whose `(generic, key)` is in `renames`.
fn rewrite_all_mono_call_sites(
    module: &mut CoreModule,
    renames: &HashMap<(String, MonoKey), String>,
) {
    // Take bodies out first so one FunIndex can borrow the signature table immutably.
    let mut functions = std::mem::take(&mut module.functions);
    let empty = Block {
        params: vec![],
        ops: vec![],
        result: None,
    };
    let mut bodies: Vec<Block> = functions
        .iter_mut()
        .map(|f| std::mem::replace(&mut f.body, empty.clone()))
        .collect();
    {
        let index = FunIndex::new(&functions, &module.sum_max_arity);
        let no_funrefs = HashMap::default();
        let no_slot_funrefs = HashMap::default();
        for i in 0..functions.len() {
            let mut local_tys: HashMap<u32, Type> = HashMap::default();
            for (j, p) in functions[i].params.iter().enumerate() {
                local_tys.insert(
                    p.0,
                    functions[i].param_tys.get(j).cloned().unwrap_or(Type::Int),
                );
            }
            let mut slot_tys: HashMap<String, Type> = HashMap::default();
            let mut int_consts: HashMap<u32, i64> = HashMap::default();
            rewrite_mono_block(
                &mut bodies[i],
                &mut local_tys,
                &mut slot_tys,
                &mut int_consts,
                renames,
                &no_funrefs,
                &no_slot_funrefs,
                &index,
            );
        }
    }
    for (fun, body) in functions.iter_mut().zip(bodies) {
        fun.body = body;
    }
    module.functions = functions;
}

fn refresh_erased_mono_return_types(module: &mut CoreModule) {
    // Analyze immutably first so we need not clone the whole function table.
    let upgrades: Vec<(usize, Type)> = {
        let snap = &module.functions;
        let traits = &module.trait_methods;
        snap.iter()
            .enumerate()
            .filter_map(|(i, fun)| {
                let params = param_ty_map(fun);
                let t = block_result_fixed_ty(&fun.body, snap, traits, &params)?;
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

/// One scan→clone pass. Returns true if any new clone was appended.
fn specialize_mono_round(
    module: &mut CoreModule,
    renames: &mut HashMap<(String, MonoKey), String>,
) -> bool {
    let index = FunIndex::new(&module.functions, &module.sum_max_arity);
    let mut needed: FxHashSet<(String, MonoKey)> = FxHashSet::default();
    for fun in &module.functions {
        let mut local_tys: HashMap<u32, Type> = HashMap::default();
        for (i, p) in fun.params.iter().enumerate() {
            local_tys.insert(p.0, fun.param_tys.get(i).cloned().unwrap_or(Type::Int));
        }
        let mut slot_tys: HashMap<String, Type> = HashMap::default();
        let mut int_consts: HashMap<u32, i64> = HashMap::default();
        scan_mono_block(
            &fun.body,
            &mut local_tys,
            &mut slot_tys,
            &mut int_consts,
            &index,
            &mut needed,
            &HashMap::default(),
            &HashMap::default(),
        );
    }

    let mut clones = Vec::new();
    let mut clone_names: FxHashSet<String> = FxHashSet::default();
    for (name, key) in needed {
        if !key.worth_cloning() {
            continue;
        }
        let Some(orig) = index.get(&name) else {
            continue;
        };
        // Do not specialize an existing mono clone (structured; `$` is only the name suffix).
        if orig.is_mono_clone() {
            continue;
        }
        if orig.is_main || orig.external.is_some() || orig.params.is_empty() {
            continue;
        }
        // Scheme-driven: monomorphic tops stay shared; FunRef HOF still clones.
        let hof = key.0.iter().any(|k| matches!(k, MonoKind::FunRef(_)));
        if !orig.scheme_poly && !hof {
            continue;
        }
        if orig.params.len() != key.0.len() {
            continue;
        }
        let new_name = format!("{name}{}", key.suffix());
        if renames.contains_key(&(name.clone(), key.clone()))
            || index.contains(&new_name)
            || clone_names.contains(&new_name)
        {
            renames.insert((name, key), new_name);
            continue;
        }
        // Call-site ABI often types heap products/lists as `Int`. Prefer the
        // generic's structural formals when materializing clone `param_tys` so
        // `AdtField` keeps Float/List params (otherwise float arith `sitofp`s
        // IEEE bit patterns — D2 `learnSteps`).
        let param_tys = materialize_mono_param_tys(&key, &orig.param_tys, index.funs());
        let inferred = key.ret_ty(index.funs());
        let binds = key.funref_param_binds(&orig.params);
        let mut clone = orig.clone();
        clone.name = new_name.clone();
        clone.mono_of = Some(name.clone());
        clone.param_tys = param_tys.clone();
        clone.memo = None;
        clone.scheme_poly = false;
        if !binds.is_empty() {
            // Directize before ret_ty: `apply(dbl, 1.5)` body becomes
            // `Call(dbl$Float, …)` whose ret is Float, not the erased Int FunRef.
            directize_block(&mut clone.body, &binds);
        }
        let ret_ty = mono_clone_ret_ty(&clone, &inferred, index.funs(), &module.trait_methods);
        if orig.param_tys == param_tys && orig.ret_ty == ret_ty && binds.is_empty() {
            continue;
        }
        clone.ret_ty = ret_ty;
        clone_names.insert(new_name.clone());
        renames.insert((name, key), new_name);
        clones.push(clone);
    }
    let added = !clones.is_empty();
    module.functions.append(&mut clones);
    added
}

/// Ret type for a mono clone: prefer body structure + formals; Num poly
/// (`{ x -> x + x }`) falls back to MonoKey when the body has no fixed ret.
fn mono_clone_ret_ty(
    fun: &CoreFun,
    inferred: &Type,
    functions: &[CoreFun],
    trait_methods: &HashMap<(String, String), Vec<String>>,
) -> Type {
    let param_map = param_ty_map(fun);
    if let Some(t) = block_result_fixed_ty(&fun.body, functions, trait_methods, &param_map) {
        return t;
    }
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
            | Type::String
            | Type::Bool => fun.ret_ty.clone(),
            _ => inferred.clone(),
        },
        Type::Adt { .. }
        | Type::List(_)
        | Type::Map(_, _)
        | Type::Set(_)
        | Type::Tuple(_)
        | Type::TuplePrefix(_) => refine_mono_container_ret(&fun.ret_ty, inferred),
        _ => inferred.clone(),
    }
}

/// Call-site ret while scanning: like [`mono_clone_ret_ty`] without walking the
/// body. Avoids `bump(Parts, Float)` → MonoKey last-arg `Float` poisoning the
/// `var p` slot (next call would mono as `$Float`).
fn call_site_mono_ret(fun: &CoreFun, inferred: &Type) -> Type {
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
            | Type::String
            | Type::Bool => fun.ret_ty.clone(),
            _ => inferred.clone(),
        },
        Type::Adt { .. }
        | Type::List(_)
        | Type::Map(_, _)
        | Type::Set(_)
        | Type::Tuple(_)
        | Type::TuplePrefix(_) => refine_mono_container_ret(&fun.ret_ty, inferred),
        _ => inferred.clone(),
    }
}

fn scan_mono_block(
    block: &Block,
    local_tys: &mut HashMap<u32, Type>,
    slot_tys: &mut HashMap<String, Type>,
    int_consts: &mut HashMap<u32, i64>,
    index: &FunIndex<'_>,
    needed: &mut FxHashSet<(String, MonoKey)>,
    parent_funrefs: &HashMap<u32, String>,
    parent_slot_funrefs: &HashMap<String, String>,
) {
    let mut funref_of = parent_funrefs.clone();
    let mut slot_funrefs = parent_slot_funrefs.clone();
    for op in &block.ops {
        match op {
            Op::Let { local, value, .. } => {
                note_mono_call(value, local_tys, index, needed, &funref_of);
                let ty = mono_value_ty_with_funrefs(
                    value, local_tys, slot_tys, int_consts, index, &funref_of,
                );
                local_tys.insert(local.0, ty);
                if let Value::Int(n) = value {
                    int_consts.insert(local.0, *n);
                } else {
                    int_consts.remove(&local.0);
                }
                match value {
                    Value::FunRef(name) => {
                        funref_of.insert(local.0, name.clone());
                    }
                    Value::Local(Local(src)) => {
                        if let Some(n) = funref_of.get(src).cloned() {
                            funref_of.insert(local.0, n);
                        } else {
                            funref_of.remove(&local.0);
                        }
                    }
                    Value::Name(n) => {
                        if let Some(fr) = slot_funrefs.get(n).cloned() {
                            funref_of.insert(local.0, fr);
                        } else {
                            funref_of.remove(&local.0);
                        }
                    }
                    _ => {
                        funref_of.remove(&local.0);
                    }
                }
                walk_mono_nested_scan(
                    value,
                    local_tys,
                    slot_tys,
                    int_consts,
                    index,
                    needed,
                    &funref_of,
                    &slot_funrefs,
                );
            }
            Op::Assign { name, value } => {
                if let Some(ty) = local_tys.get(&value.0).cloned() {
                    slot_tys.insert(name.clone(), ty);
                }
                if let Some(fr) = funref_of.get(&value.0).cloned() {
                    slot_funrefs.insert(name.clone(), fr);
                } else {
                    slot_funrefs.remove(name);
                }
            }
            Op::Effect { value } => {
                note_mono_call(value, local_tys, index, needed, &funref_of);
                walk_mono_nested_scan(
                    value,
                    local_tys,
                    slot_tys,
                    int_consts,
                    index,
                    needed,
                    &funref_of,
                    &slot_funrefs,
                );
            }
            _ => {}
        }
    }
}

fn walk_mono_nested_scan(
    value: &Value,
    local_tys: &mut HashMap<u32, Type>,
    slot_tys: &mut HashMap<String, Type>,
    int_consts: &mut HashMap<u32, i64>,
    index: &FunIndex<'_>,
    needed: &mut FxHashSet<(String, MonoKey)>,
    funref_of: &HashMap<u32, String>,
    slot_funrefs: &HashMap<String, String>,
) {
    crate::for_each_nested_block(value, &mut |b| {
        scan_mono_block(
            b,
            local_tys,
            slot_tys,
            int_consts,
            index,
            needed,
            funref_of,
            slot_funrefs,
        );
    });
}

/// True when `fun` already names a mono clone registered in the index.
fn callee_is_mono_clone(fun: &str, index: &FunIndex<'_>) -> bool {
    index.get(fun).is_some_and(|f| f.is_mono_clone())
}

fn note_mono_call(
    value: &Value,
    local_tys: &HashMap<u32, Type>,
    index: &FunIndex<'_>,
    needed: &mut FxHashSet<(String, MonoKey)>,
    funref_of: &HashMap<u32, String>,
) {
    match value {
        Value::Call { fun, args } => {
            if args.is_empty() || callee_is_mono_clone(fun, index) {
                return;
            }
            let Some(f) = index.get(fun) else {
                return;
            };
            let Some(key) = args_mono_key(args, local_tys, funref_of, Some(f.param_tys.as_slice()))
            else {
                return;
            };
            note_needed_clone(fun, key, f, index, needed);
        }
        // Parallel list HOFs pass FunRef callbacks as i64 ABI workers. Without
        // specializing `__lam_*` to Float, codegen emits Int `+` on IEEE bits.
        Value::Builtin {
            name: Builtin::ListParMap,
            args,
        } if args.len() == 2 => {
            let Some(cb) = funref_of.get(&args[1].0) else {
                return;
            };
            let Some(Type::List(elem)) = local_tys.get(&args[0].0) else {
                return;
            };
            let Some(key) = types_mono_key(&[elem.as_ref().clone()]) else {
                return;
            };
            let Some(f) = index.get(cb) else {
                return;
            };
            note_needed_clone(cb, key, f, index, needed);
        }
        Value::Builtin {
            name: Builtin::ListParFold,
            args,
        } if args.len() == 3 => {
            let Some(cb) = funref_of.get(&args[2].0) else {
                return;
            };
            let Some(Type::List(elem)) = local_tys.get(&args[0].0) else {
                return;
            };
            let Some(init_ty) = local_tys.get(&args[1].0) else {
                return;
            };
            let Some(key) = types_mono_key(&[init_ty.clone(), elem.as_ref().clone()]) else {
                return;
            };
            let Some(f) = index.get(cb) else {
                return;
            };
            note_needed_clone(cb, key, f, index, needed);
        }
        _ => {}
    }
}

fn note_needed_clone(
    fun: &str,
    key: MonoKey,
    f: &CoreFun,
    index: &FunIndex<'_>,
    needed: &mut FxHashSet<(String, MonoKey)>,
) {
    if !key.worth_cloning() {
        return;
    }
    if f.params.len() != key.0.len() {
        return;
    }
    let funs = index.funs();
    let param_tys = materialize_mono_param_tys(&key, &f.param_tys, funs);
    let ret = key.ret_ty(funs);
    if f.param_tys == param_tys && f.ret_ty == ret && key.funref_param_binds(&f.params).is_empty() {
        return;
    }
    needed.insert((fun.to_string(), key));
}

pub(crate) fn mono_value_ty(
    value: &Value,
    local_tys: &HashMap<u32, Type>,
    slot_tys: &HashMap<String, Type>,
    int_consts: &HashMap<u32, i64>,
    index: &FunIndex<'_>,
) -> Type {
    mono_value_ty_with_funrefs(
        value,
        local_tys,
        slot_tys,
        int_consts,
        index,
        &HashMap::default(),
    )
}

fn mono_value_ty_with_funrefs(
    value: &Value,
    local_tys: &HashMap<u32, Type>,
    slot_tys: &HashMap<String, Type>,
    int_consts: &HashMap<u32, i64>,
    index: &FunIndex<'_>,
    funref_of: &HashMap<u32, String>,
) -> Type {
    let funs = index.funs();
    let mut call_ret = |fun: &str, args: &[Local]| -> Option<Type> {
        let formals = index.get(fun).map(|f| f.param_tys.as_slice());
        // Prefer call-site mono key so `dbl(1.5)` types as Float before the
        // `dbl$Float` clone exists (ListAppend / fold otherwise keep List[Int]).
        if let Some(key) = args_mono_key(args, local_tys, funref_of, formals) {
            if key.worth_cloning() || callee_is_mono_clone(fun, index) {
                let inferred = key.ret_ty(funs);
                if let Some(f) = index.get(fun) {
                    return Some(call_site_mono_ret(f, &inferred));
                }
                return Some(inferred);
            }
        }
        if let Some(f) = index.get(fun) {
            return Some(f.ret_ty.clone());
        }
        if callee_is_mono_clone(fun, index) {
            if let Some(key) = args_mono_key(args, local_tys, funref_of, None) {
                return Some(key.ret_ty(funs));
            }
        }
        None
    };
    // Thread FunRef names so ListParMap can read callback ret via funref_locals.
    let mut fun_ret_tys: HashMap<String, Type> = HashMap::default();
    let mut funref_locals: HashMap<u32, String> = HashMap::default();
    for (loc, name) in funref_of {
        funref_locals.insert(*loc, name.clone());
        if let Some(f) = index.get(name) {
            // If a mono key would upgrade ret (e.g. pending Float clone), prefer that
            // once the clone exists; until then use generic ret — list_elem fallback
            // still keeps List[Float] for float source lists.
            fun_ret_tys.insert(name.clone(), f.ret_ty.clone());
        }
    }
    infer_value_ty_ctx(
        value,
        InferValueCtx {
            local_tys,
            slot_tys: Some(slot_tys),
            fun_ret_tys: Some(&fun_ret_tys),
            fun_param_tys: None,
            fun_param0_identity: None,
            funref_locals: Some(&funref_locals),
            local_int_consts: Some(int_consts),
            sum_max_arity: Some(index.sum_max_arity),
        },
        Some(&mut call_ret),
    )
}

fn rewrite_mono_block(
    block: &mut Block,
    local_tys: &mut HashMap<u32, Type>,
    slot_tys: &mut HashMap<String, Type>,
    int_consts: &mut HashMap<u32, i64>,
    renames: &HashMap<(String, MonoKey), String>,
    parent_funrefs: &HashMap<u32, String>,
    parent_slot_funrefs: &HashMap<String, String>,
    index: &FunIndex<'_>,
) {
    let mut funref_of = parent_funrefs.clone();
    let mut slot_funrefs = parent_slot_funrefs.clone();
    for i in 0..block.ops.len() {
        let (before, rest) = block.ops.split_at_mut(i);
        let op = &mut rest[0];
        match op {
            Op::Let { local, value, .. } => {
                let patch = par_hof_funref_patch(value, local_tys, renames, &funref_of);
                rewrite_mono_value(
                    value,
                    local_tys,
                    slot_tys,
                    int_consts,
                    renames,
                    &funref_of,
                    &slot_funrefs,
                    index,
                );
                if let Some((cb_local, new_name)) = patch {
                    patch_funref_let(before, cb_local, &new_name);
                    funref_of.insert(cb_local, new_name);
                }
                let ty = mono_value_ty_rewrite(
                    value, local_tys, slot_tys, int_consts, renames, &funref_of, index,
                );
                local_tys.insert(local.0, ty);
                if let Value::Int(n) = value {
                    int_consts.insert(local.0, *n);
                } else {
                    int_consts.remove(&local.0);
                }
                match value {
                    Value::FunRef(name) => {
                        funref_of.insert(local.0, name.clone());
                    }
                    Value::Local(Local(src)) => {
                        if let Some(n) = funref_of.get(src).cloned() {
                            funref_of.insert(local.0, n);
                        } else {
                            funref_of.remove(&local.0);
                        }
                    }
                    Value::Name(n) => {
                        if let Some(fr) = slot_funrefs.get(n).cloned() {
                            funref_of.insert(local.0, fr);
                        } else {
                            funref_of.remove(&local.0);
                        }
                    }
                    _ => {
                        funref_of.remove(&local.0);
                    }
                }
            }
            Op::Assign { name, value } => {
                if let Some(ty) = local_tys.get(&value.0).cloned() {
                    slot_tys.insert(name.clone(), ty);
                }
                if let Some(fr) = funref_of.get(&value.0).cloned() {
                    slot_funrefs.insert(name.clone(), fr);
                } else {
                    slot_funrefs.remove(name);
                }
            }
            Op::Effect { value } => {
                let patch = par_hof_funref_patch(value, local_tys, renames, &funref_of);
                rewrite_mono_value(
                    value,
                    local_tys,
                    slot_tys,
                    int_consts,
                    renames,
                    &funref_of,
                    &slot_funrefs,
                    index,
                );
                if let Some((cb_local, new_name)) = patch {
                    patch_funref_let(before, cb_local, &new_name);
                    funref_of.insert(cb_local, new_name);
                }
            }
            _ => {}
        }
    }
}

fn par_hof_funref_patch(
    value: &Value,
    local_tys: &HashMap<u32, Type>,
    renames: &HashMap<(String, MonoKey), String>,
    funref_of: &HashMap<u32, String>,
) -> Option<(u32, String)> {
    match value {
        Value::Builtin {
            name: Builtin::ListParMap,
            args,
        } if args.len() == 2 => rewrite_par_hof_funref(
            args[1].0,
            &list_elem_ty(local_tys, args[0].0),
            renames,
            funref_of,
        ),
        Value::Builtin {
            name: Builtin::ListParFold,
            args,
        } if args.len() == 3 => {
            let mut tys = Vec::new();
            if let Some(t) = local_tys.get(&args[1].0) {
                tys.push(t.clone());
            }
            if let Some(Type::List(e)) = local_tys.get(&args[0].0) {
                tys.push(e.as_ref().clone());
            }
            rewrite_par_hof_funref(args[2].0, &tys, renames, funref_of)
        }
        _ => None,
    }
}

fn patch_funref_let(ops: &mut [Op], local: u32, new_name: &str) {
    for op in ops {
        if let Op::Let {
            local: l,
            value: Value::FunRef(n),
            ..
        } = op
        {
            if l.0 == local {
                *n = new_name.to_string();
                return;
            }
        }
    }
}

fn rewrite_mono_value(
    value: &mut Value,
    local_tys: &mut HashMap<u32, Type>,
    slot_tys: &mut HashMap<String, Type>,
    int_consts: &mut HashMap<u32, i64>,
    renames: &HashMap<(String, MonoKey), String>,
    funref_of: &HashMap<u32, String>,
    slot_funrefs: &HashMap<String, String>,
    index: &FunIndex<'_>,
) {
    match value {
        Value::Call { fun, args } => {
            if args.is_empty() || callee_is_mono_clone(fun, index) {
                return;
            }
            let formals = index.get(fun).map(|f| f.param_tys.as_slice());
            if let Some(key) = args_mono_key(args, local_tys, funref_of, formals) {
                if let Some(new) = renames.get(&(fun.clone(), key)) {
                    *fun = new.clone();
                }
            }
        }
        _ => {
            crate::for_each_nested_block_mut(value, &mut |b| {
                rewrite_mono_block(
                    b,
                    local_tys,
                    slot_tys,
                    int_consts,
                    renames,
                    funref_of,
                    slot_funrefs,
                    index,
                );
            });
        }
    }
}

fn list_elem_ty(local_tys: &HashMap<u32, Type>, list: u32) -> Vec<Type> {
    match local_tys.get(&list) {
        Some(Type::List(e)) => vec![e.as_ref().clone()],
        _ => vec![],
    }
}

fn rewrite_par_hof_funref(
    cb_local: u32,
    cb_param_tys: &[Type],
    renames: &HashMap<(String, MonoKey), String>,
    funref_of: &HashMap<u32, String>,
) -> Option<(u32, String)> {
    let cb = funref_of.get(&cb_local)?;
    let key = types_mono_key(cb_param_tys)?;
    let new = renames.get(&(cb.clone(), key))?;
    Some((cb_local, new.clone()))
}

fn mono_value_ty_rewrite(
    value: &Value,
    local_tys: &HashMap<u32, Type>,
    slot_tys: &HashMap<String, Type>,
    int_consts: &HashMap<u32, i64>,
    renames: &HashMap<(String, MonoKey), String>,
    funref_of: &HashMap<u32, String>,
    index: &FunIndex<'_>,
) -> Type {
    let funs = index.funs();
    match value {
        Value::Call { fun, args } => {
            if let Some(((_, mk), _)) = renames.iter().find(|(_, n)| *n == fun) {
                return mk.ret_ty(funs);
            }
            let formals = index.get(fun).map(|f| f.param_tys.as_slice());
            if let Some(key) = args_mono_key(args, local_tys, funref_of, formals) {
                if let Some(new) = renames.get(&(fun.clone(), key.clone())) {
                    if let Some(((_, mk), _)) = renames.iter().find(|(_, n)| *n == new) {
                        return mk.ret_ty(funs);
                    }
                }
                if callee_is_mono_clone(fun, index) || key.worth_cloning() {
                    return key.ret_ty(funs);
                }
            }
            if let Some(f) = index.get(fun) {
                return f.ret_ty.clone();
            }
            Type::Int
        }
        other => mono_value_ty(other, local_tys, slot_tys, int_consts, index),
    }
}

/// Mono FunRef toehold: if a clone's result is `Call(target, params)` (pure
/// forwarder), rewrite call sites to `target` so bodies are shared in practice.
fn elide_trivial_mono_forwarders(module: &mut CoreModule) {
    let mut forward: HashMap<String, String> = HashMap::default();
    for f in &module.functions {
        if let Some(target) = trivial_param_forward_target(f) {
            if target != f.name {
                forward.insert(f.name.clone(), target);
            }
        }
    }
    if forward.is_empty() {
        return;
    }
    // Collapse chains A→B→C.
    let keys: Vec<String> = forward.keys().cloned().collect();
    for k in keys {
        let mut cur = forward.get(&k).cloned();
        let mut guard = 0;
        while let Some(ref t) = cur {
            if let Some(next) = forward.get(t) {
                cur = Some(next.clone());
                guard += 1;
                if guard > 8 {
                    break;
                }
            } else {
                break;
            }
        }
        if let Some(t) = cur {
            forward.insert(k, t);
        }
    }
    for fun in &mut module.functions {
        rewrite_forward_calls(&mut fun.body, &forward);
    }
}

fn trivial_param_forward_target(fun: &CoreFun) -> Option<String> {
    fun.mono_of.as_ref()?;
    let result = fun.body.result?;
    for op in &fun.body.ops {
        let Op::Let {
            local,
            value: Value::Call { fun: target, args },
            ..
        } = op
        else {
            continue;
        };
        if *local != result {
            continue;
        }
        // Exact forward of all formals (identity-shaped mono clone).
        if args.len() == fun.params.len() && args.iter().eq(fun.params.iter()) {
            return Some(target.clone());
        }
    }
    None
}

fn rewrite_forward_calls(block: &mut Block, forward: &HashMap<String, String>) {
    for op in &mut block.ops {
        match op {
            Op::Let { value, .. } | Op::Effect { value } => {
                rewrite_forward_value(value, forward);
            }
            _ => {}
        }
    }
}

fn rewrite_forward_value(value: &mut Value, forward: &HashMap<String, String>) {
    match value {
        Value::Call { fun, .. } => {
            if let Some(t) = forward.get(fun) {
                *fun = t.clone();
            }
        }
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            rewrite_forward_calls(then_block, forward);
            rewrite_forward_calls(else_block, forward);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            rewrite_forward_calls(header, forward);
            rewrite_forward_calls(body, forward);
            rewrite_forward_calls(latch, forward);
        }
        Value::Lambda { body, .. } => rewrite_forward_calls(body, forward),
        _ => {}
    }
}
