use super::fun_index::FunIndex;
use super::key::{args_mono_key, MonoKey, MonoKind};
use super::ret_ty::{block_result_fixed_ty, param_ty_map, refine_mono_container_ret};
use super::traits::directize_block;
use crate::ir::{Block, CoreFun, CoreModule, Local, Op, Value};
use crate::value_ty::infer_value_ty;
use lumia_ty::Type;
use rustc_hash::{FxHashMap as HashMap, FxHashSet};

pub(crate) fn specialize_mono_calls(module: &mut CoreModule) {
    let mut renames: HashMap<(String, MonoKey), String> = HashMap::default();
    for _round in 0..8 {
        let added = specialize_mono_round(module, &mut renames);
        if !added {
            break;
        }
    }
    if renames.is_empty() {
        return;
    }
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
        let index = FunIndex::new(&functions);
        let no_funrefs = HashMap::default();
        for i in 0..functions.len() {
            let mut local_tys: HashMap<u32, Type> = HashMap::default();
            for (j, p) in functions[i].params.iter().enumerate() {
                local_tys.insert(
                    p.0,
                    functions[i].param_tys.get(j).cloned().unwrap_or(Type::Int),
                );
            }
            rewrite_mono_block(
                &mut bodies[i],
                &mut local_tys,
                &renames,
                &no_funrefs,
                &index,
            );
        }
    }
    for (fun, body) in functions.iter_mut().zip(bodies) {
        fun.body = body;
    }
    module.functions = functions;
    // After all clones exist, upgrade erased Int rets on HOF wrappers whose
    // bodies now `Call(dbl$Float, …)` (directize order within a round varies).
    refresh_body_fixed_ret_tys(module);
}

fn refresh_body_fixed_ret_tys(module: &mut CoreModule) {
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
    let index = FunIndex::new(&module.functions);
    let mut needed: FxHashSet<(String, MonoKey)> = FxHashSet::default();
    for fun in &module.functions {
        let mut local_tys: HashMap<u32, Type> = HashMap::default();
        for (i, p) in fun.params.iter().enumerate() {
            local_tys.insert(p.0, fun.param_tys.get(i).cloned().unwrap_or(Type::Int));
        }
        scan_mono_block(
            &fun.body,
            &mut local_tys,
            &index,
            &mut needed,
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
        let param_tys = key.param_tys(index.funs());
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

fn scan_mono_block(
    block: &Block,
    local_tys: &mut HashMap<u32, Type>,
    index: &FunIndex<'_>,
    needed: &mut FxHashSet<(String, MonoKey)>,
    parent_funrefs: &HashMap<u32, String>,
) {
    let mut funref_of = parent_funrefs.clone();
    for op in &block.ops {
        match op {
            Op::Let { local, value, .. } => {
                note_mono_call(value, local_tys, index, needed, &funref_of);
                let ty = mono_value_ty(value, local_tys, index);
                local_tys.insert(local.0, ty);
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
                    _ => {
                        funref_of.remove(&local.0);
                    }
                }
                walk_mono_nested_scan(value, local_tys, index, needed, &funref_of);
            }
            Op::Effect { value } => {
                note_mono_call(value, local_tys, index, needed, &funref_of);
                walk_mono_nested_scan(value, local_tys, index, needed, &funref_of);
            }
            _ => {}
        }
    }
}

fn walk_mono_nested_scan(
    value: &Value,
    local_tys: &mut HashMap<u32, Type>,
    index: &FunIndex<'_>,
    needed: &mut FxHashSet<(String, MonoKey)>,
    funref_of: &HashMap<u32, String>,
) {
    match value {
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            scan_mono_block(then_block, local_tys, index, needed, funref_of);
            scan_mono_block(else_block, local_tys, index, needed, funref_of);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            scan_mono_block(header, local_tys, index, needed, funref_of);
            scan_mono_block(body, local_tys, index, needed, funref_of);
            scan_mono_block(latch, local_tys, index, needed, funref_of);
        }
        _ => {}
    }
}

/// True when `fun` already names a mono clone (or still uses legacy `$` suffix only).
fn callee_is_mono_clone(fun: &str, index: &FunIndex<'_>) -> bool {
    index
        .get(fun)
        .map(|f| f.is_mono_clone())
        .unwrap_or_else(|| fun.contains('$'))
}

fn note_mono_call(
    value: &Value,
    local_tys: &HashMap<u32, Type>,
    index: &FunIndex<'_>,
    needed: &mut FxHashSet<(String, MonoKey)>,
    funref_of: &HashMap<u32, String>,
) {
    let Value::Call { fun, args } = value else {
        return;
    };
    if args.is_empty() || callee_is_mono_clone(fun, index) {
        return;
    }
    let Some(key) = args_mono_key(args, local_tys, funref_of) else {
        return;
    };
    if !key.worth_cloning() {
        return;
    }
    let Some(f) = index.get(fun) else {
        return;
    };
    if f.params.len() != key.0.len() {
        return;
    }
    let funs = index.funs();
    let param_tys = key.param_tys(funs);
    let ret = key.ret_ty(funs);
    if f.param_tys == param_tys && f.ret_ty == ret && key.funref_param_binds(&f.params).is_empty() {
        return;
    }
    needed.insert((fun.clone(), key));
}

pub(crate) fn mono_value_ty(
    value: &Value,
    local_tys: &HashMap<u32, Type>,
    index: &FunIndex<'_>,
) -> Type {
    let funs = index.funs();
    infer_value_ty(value, local_tys, |fun, args| {
        if let Some(f) = index.get(fun) {
            return Some(f.ret_ty.clone());
        }
        // Clone not yet indexed this round — recover ret from the call-site key.
        if callee_is_mono_clone(fun, index) {
            if let Some(key) = args_mono_key(args, local_tys, &HashMap::default()) {
                return Some(key.ret_ty(funs));
            }
        }
        None
    })
}

fn rewrite_mono_block(
    block: &mut Block,
    local_tys: &mut HashMap<u32, Type>,
    renames: &HashMap<(String, MonoKey), String>,
    parent_funrefs: &HashMap<u32, String>,
    index: &FunIndex<'_>,
) {
    let mut funref_of = parent_funrefs.clone();
    for op in &mut block.ops {
        match op {
            Op::Let { local, value, .. } => {
                rewrite_mono_value(value, local_tys, renames, &funref_of, index);
                let ty = mono_value_ty_rewrite(value, local_tys, renames, &funref_of, index);
                local_tys.insert(local.0, ty);
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
                    _ => {
                        funref_of.remove(&local.0);
                    }
                }
            }
            Op::Effect { value } => {
                rewrite_mono_value(value, local_tys, renames, &funref_of, index)
            }
            _ => {}
        }
    }
}

fn rewrite_mono_value(
    value: &mut Value,
    local_tys: &mut HashMap<u32, Type>,
    renames: &HashMap<(String, MonoKey), String>,
    funref_of: &HashMap<u32, String>,
    index: &FunIndex<'_>,
) {
    match value {
        Value::Call { fun, args } => {
            if args.is_empty() || callee_is_mono_clone(fun, index) {
                return;
            }
            if let Some(key) = args_mono_key(args, local_tys, funref_of) {
                if let Some(new) = renames.get(&(fun.clone(), key)) {
                    *fun = new.clone();
                }
            }
        }
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            rewrite_mono_block(then_block, local_tys, renames, funref_of, index);
            rewrite_mono_block(else_block, local_tys, renames, funref_of, index);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            rewrite_mono_block(header, local_tys, renames, funref_of, index);
            rewrite_mono_block(body, local_tys, renames, funref_of, index);
            rewrite_mono_block(latch, local_tys, renames, funref_of, index);
        }
        _ => {}
    }
}

fn mono_value_ty_rewrite(
    value: &Value,
    local_tys: &HashMap<u32, Type>,
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
            if let Some(key) = args_mono_key(args, local_tys, funref_of) {
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
            // Legacy name-suffix fallback when the clone is not yet in the index.
            if fun.ends_with("$Float") {
                return Type::Float;
            }
            if fun.ends_with("$Bool") {
                return Type::Bool;
            }
            if fun.ends_with("$String") {
                return Type::String;
            }
            Type::Int
        }
        other => mono_value_ty(other, local_tys, index),
    }
}
