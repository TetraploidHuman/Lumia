use super::super::directize::directize_block;
use super::super::fun_index::FunIndex;
use super::super::key::{
    args_mono_key, materialize_mono_param_tys, types_mono_key, MonoKey, MonoKind,
};
use crate::ir::{Block, CoreFun, CoreModule, Local, Op, Value};
use crate::value_ty::{infer_value_ty_ctx, InferValueCtx};
use crate::visit::for_each_top_level_op_in_block;
use lumia_hir::Builtin;
use lumia_syntax::Sym;
use lumia_ty::{Effect, Type};
use rustc_hash::{FxHashMap as HashMap, FxHashSet};
use std::sync::Arc;

use super::funref::FunrefSlots;
use super::ret_refresh::{call_site_mono_ret, mono_clone_ret_ty, mono_clone_ret_ty_parts};
use super::rewrite::track_funref_after_let;

pub(super) fn args_mono_key_idx(
    args: &[Local],
    local_tys: &HashMap<u32, Type>,
    funref_of: &HashMap<u32, String>,
    formals: Option<&[Type]>,
    index: &FunIndex<'_>,
) -> Option<MonoKey> {
    let mut key = args_mono_key(args, local_tys, funref_of, formals)?;
    index.stamp_funref_ids(&mut key);
    Some(key)
}

/// Max clone-discovery iterations.
///
/// Transitive FunRef HOF chains (`optMap` → `apply` → `dbl`) typically need 2–3
/// rounds. This cap is a safety fuse against non-termination bugs; the loop
/// converges early when a round adds no clones.
const MAX_MONO_CLONE_ROUNDS: usize = lumia_abi::MONO_CLONE_ROUNDS;

/// Fixed-point: scan all bodies for needed `(generic, MonoKey)` clones, append
/// them, repeat until the worklist is empty or [`MAX_MONO_CLONE_ROUNDS`] hits.
pub(super) fn collect_mono_clones_until_fixed_point(
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

fn specialize_mono_round(
    module: &mut CoreModule,
    renames: &mut HashMap<(String, MonoKey), String>,
) -> bool {
    let index = FunIndex::new(
        &module.functions,
        &module.sum_max_arity,
        &module.trait_methods,
        module.channel_elem_hint.as_ref(),
    );
    let mut needed: FxHashSet<(String, MonoKey)> = FxHashSet::default();
    for fun in &module.functions {
        let mut local_tys: HashMap<u32, Type> = HashMap::default();
        for (i, p) in fun.params.iter().enumerate() {
            local_tys.insert(p.0, fun.param_tys.get(i).cloned().unwrap_or(Type::Int));
        }
        let mut slot_tys: HashMap<Sym, Type> = HashMap::default();
        let mut int_consts: HashMap<u32, i64> = HashMap::default();
        // Shared across nested Loop/If like `slot_tys` so flatMap's mut acc
        // (`ListConcat` in the loop body) is visible to post-loop `ListGet`.
        let mut slot_list_funrefs: HashMap<Sym, FunrefSlots> = HashMap::default();
        let mut slot_adt_funrefs: HashMap<Sym, FunrefSlots> = HashMap::default();
        let mut list_funrefs: HashMap<u32, FunrefSlots> = HashMap::default();
        let mut adt_funrefs: HashMap<u32, FunrefSlots> = HashMap::default();
        let mut bool_consts: HashMap<u32, bool> = HashMap::default();
        let mut adt_tags: HashMap<u32, i64> = HashMap::default();
        scan_mono_block(
            &fun.body,
            &mut local_tys,
            &mut slot_tys,
            &mut int_consts,
            &mut bool_consts,
            &index,
            &mut needed,
            &HashMap::default(),
            &HashMap::default(),
            &mut slot_list_funrefs,
            &mut slot_adt_funrefs,
            &mut list_funrefs,
            &mut adt_funrefs,
            &mut adt_tags,
        );
    }

    let mut clones = Vec::new();
    let mut clone_names: FxHashSet<String> = FxHashSet::default();
    // One Arc<body> per original name this round — shared across many MonoKeys.
    let mut body_arcs: HashMap<String, Arc<Block>> = HashMap::default();
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
        let inferred = key.ret_ty(index.funs(), Some(name.as_str()));
        let binds = key.funref_param_binds(&orig.params);

        // Probe ret / eligibility before materializing a body clone when there
        // is no FunRef directize (discard path used to clone then drop).
        if binds.is_empty() {
            let ret_ty = mono_clone_ret_ty_parts(
                &orig.body,
                &orig.params,
                &param_tys,
                &orig.ret_ty,
                &inferred,
                &index,
            );
            if orig.param_tys == param_tys && orig.ret_ty == ret_ty {
                continue;
            }
            let body_arc = body_arcs
                .entry(name.clone())
                .or_insert_with(|| Arc::new(orig.body.clone()))
                .clone();
            let clone = CoreFun {
                name: new_name.clone().into(),
                params: orig.params.clone(),
                param_names: orig.param_names.clone(),
                param_tys: param_tys.clone(),
                body: (*body_arc).clone(),
                ret_ty,
                effect: orig.effect,
                is_main: false,
                memo: None,
                external: None,
                foreign_abi: orig.foreign_abi,
                escaping: Default::default(),
                nsw_binop_locals: Default::default(),
                safe_divisor_locals: Default::default(),
                nonneg_iv_load_locals: Default::default(),
                scheme_poly: false,
                mono_of: Some(name.clone()),
                kind: orig.kind,
            };
            clone_names.insert(new_name.clone());
            renames.insert((name, key), new_name);
            clones.push(clone);
            continue;
        }

        let body_arc = body_arcs
            .entry(name.clone())
            .or_insert_with(|| Arc::new(orig.body.clone()))
            .clone();
        let mut body = (*body_arc).clone();
        // Directize before ret_ty: `apply(dbl, 1.5)` body becomes
        // `Call(dbl$Float, …)` whose ret is Float, not the erased Int FunRef.
        directize_block(&mut body, &binds);
        let mut clone = CoreFun {
            name: new_name.clone().into(),
            params: orig.params.clone(),
            param_names: orig.param_names.clone(),
            param_tys: param_tys.clone(),
            body,
            ret_ty: orig.ret_ty.clone(),
            effect: orig.effect,
            is_main: false,
            memo: None,
            external: None,
            foreign_abi: orig.foreign_abi,
            escaping: Default::default(),
            nsw_binop_locals: Default::default(),
            safe_divisor_locals: Default::default(),
            nonneg_iv_load_locals: Default::default(),
            scheme_poly: false,
            mono_of: Some(name.clone()),
            kind: orig.kind,
        };
        clone.ret_ty = mono_clone_ret_ty(&clone, &inferred, &index);
        clone_names.insert(new_name.clone());
        renames.insert((name, key), new_name);
        clones.push(clone);
    }
    let added = !clones.is_empty();
    module.functions.append(&mut clones);
    added
}

fn scan_mono_block(
    block: &Block,
    local_tys: &mut HashMap<u32, Type>,
    slot_tys: &mut HashMap<Sym, Type>,
    int_consts: &mut HashMap<u32, i64>,
    bool_consts: &mut HashMap<u32, bool>,
    index: &FunIndex<'_>,
    needed: &mut FxHashSet<(String, MonoKey)>,
    parent_funrefs: &HashMap<u32, String>,
    parent_slot_funrefs: &HashMap<Sym, String>,
    slot_list_funrefs: &mut HashMap<Sym, FunrefSlots>,
    slot_adt_funrefs: &mut HashMap<Sym, FunrefSlots>,
    list_funrefs: &mut HashMap<u32, FunrefSlots>,
    adt_funrefs: &mut HashMap<u32, FunrefSlots>,
    adt_tags: &mut HashMap<u32, i64>,
) {
    let mut funref_of = parent_funrefs.clone();
    let mut slot_funrefs = parent_slot_funrefs.clone();
    // Task local → spawned FunRef/AllocClosure name (for join → FunRef chase).
    let mut spawn_of: HashMap<u32, String> = HashMap::default();
    for_each_top_level_op_in_block(block, &mut |op| match op {
        Op::Let { local, value, .. } => {
            // Nested If/Loop arms first so `If` result can join arm locals
            // (`opt alt listOf(0.0)` → List[Float]). Typing before the walk
            // left If as Int and skipped ListParFold Float mono clones.
            walk_mono_nested_scan(
                value,
                local_tys,
                slot_tys,
                int_consts,
                bool_consts,
                index,
                needed,
                &funref_of,
                &slot_funrefs,
                slot_list_funrefs,
                slot_adt_funrefs,
                list_funrefs,
                adt_funrefs,
                adt_tags,
            );
            let ty = mono_value_ty_with_funrefs(
                value, local_tys, slot_tys, int_consts, index, &funref_of,
            );
            local_tys.insert(local.0, ty);
            note_scalar_consts(local.0, value, int_consts, bool_consts, adt_tags);
            track_funref_after_let(
                local.0,
                value,
                &mut funref_of,
                &mut spawn_of,
                list_funrefs,
                adt_funrefs,
                &slot_funrefs,
                slot_list_funrefs,
                slot_adt_funrefs,
                int_consts,
                bool_consts,
                None,
                None,
                None,
                Some(index),
            );
            // After nested + this let: ListParFold sees List[Float] list arg.
            note_mono_call(value, local_tys, index, needed, &funref_of);
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
            if let Some(v) = list_funrefs.get(&value.0).cloned() {
                slot_list_funrefs.insert(name.clone(), v);
            } else {
                slot_list_funrefs.remove(name);
            }
            if let Some(v) = adt_funrefs.get(&value.0).cloned() {
                slot_adt_funrefs.insert(name.clone(), v);
            } else {
                slot_adt_funrefs.remove(name);
            }
        }
        _ => {}
    });
}

pub(super) fn note_scalar_consts(
    local: u32,
    value: &Value,
    int_consts: &mut HashMap<u32, i64>,
    bool_consts: &mut HashMap<u32, bool>,
    adt_tags: &mut HashMap<u32, i64>,
) {
    match value {
        Value::Int(n) => {
            int_consts.insert(local, *n);
        }
        Value::Local(Local(src)) => {
            if let Some(n) = int_consts.get(src).copied() {
                int_consts.insert(local, n);
            } else {
                int_consts.remove(&local);
            }
        }
        Value::Builtin {
            name: Builtin::AdtTag,
            args,
            ..
        } => {
            if let Some(tag) = args.first().and_then(|a| adt_tags.get(&a.0).copied()) {
                int_consts.insert(local, tag);
            } else {
                int_consts.remove(&local);
            }
        }
        _ => {
            int_consts.remove(&local);
        }
    }
    match value {
        Value::AllocAdt { tag, .. } => {
            adt_tags.insert(local, *tag);
        }
        Value::Local(Local(src)) => {
            if let Some(t) = adt_tags.get(src).copied() {
                adt_tags.insert(local, t);
            } else {
                adt_tags.remove(&local);
            }
        }
        _ => {
            adt_tags.remove(&local);
        }
    }
    match value {
        Value::Bool(b) => {
            bool_consts.insert(local, *b);
        }
        Value::Local(Local(src)) => {
            if let Some(b) = bool_consts.get(src).copied() {
                bool_consts.insert(local, b);
            } else {
                bool_consts.remove(&local);
            }
        }
        Value::Binary {
            op: crate::CoreBinOp::Eq,
            left,
            right,
        } => match (
            int_consts.get(&left.0).copied(),
            int_consts.get(&right.0).copied(),
        ) {
            (Some(a), Some(b)) => {
                bool_consts.insert(local, a == b);
            }
            _ => {
                bool_consts.remove(&local);
            }
        },
        Value::Binary {
            op: crate::CoreBinOp::Ne,
            left,
            right,
        } => match (
            int_consts.get(&left.0).copied(),
            int_consts.get(&right.0).copied(),
        ) {
            (Some(a), Some(b)) => {
                bool_consts.insert(local, a != b);
            }
            _ => {
                bool_consts.remove(&local);
            }
        },
        _ => {
            bool_consts.remove(&local);
        }
    }
}

fn walk_mono_nested_scan(
    value: &Value,
    local_tys: &mut HashMap<u32, Type>,
    slot_tys: &mut HashMap<Sym, Type>,
    int_consts: &mut HashMap<u32, i64>,
    bool_consts: &mut HashMap<u32, bool>,
    index: &FunIndex<'_>,
    needed: &mut FxHashSet<(String, MonoKey)>,
    funref_of: &HashMap<u32, String>,
    slot_funrefs: &HashMap<Sym, String>,
    slot_list_funrefs: &mut HashMap<Sym, FunrefSlots>,
    slot_adt_funrefs: &mut HashMap<Sym, FunrefSlots>,
    list_funrefs: &mut HashMap<u32, FunrefSlots>,
    adt_funrefs: &mut HashMap<u32, FunrefSlots>,
    adt_tags: &mut HashMap<u32, i64>,
) {
    crate::for_each_nested_block(value, &mut |b| {
        scan_mono_block(
            b,
            local_tys,
            slot_tys,
            int_consts,
            bool_consts,
            index,
            needed,
            funref_of,
            slot_funrefs,
            slot_list_funrefs,
            slot_adt_funrefs,
            list_funrefs,
            adt_funrefs,
            adt_tags,
        );
    });
}

/// True when `fun` already names a mono clone registered in the index.
pub(super) fn callee_is_mono_clone(fun: &str, index: &FunIndex<'_>) -> bool {
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
            if args.is_empty() || callee_is_mono_clone(fun.as_str(), index) {
                return;
            }
            let Some(f) = index.get(fun.as_str()) else {
                return;
            };
            let Some(key) = args_mono_key_idx(
                args,
                local_tys,
                funref_of,
                Some(f.param_tys.as_slice()),
                index,
            ) else {
                return;
            };
            note_needed_clone(fun.as_str(), key, f, index, needed);
        }
        // `spawn { { x -> x } }.join()(1.5)` — FunRef survives join; specialize
        // the identity body so icall can become `Call(__lam$Float, …)`.
        Value::IndirectCall { callee, args } => {
            let Some(fun) = funref_of.get(&callee.0) else {
                return;
            };
            if args.is_empty() || callee_is_mono_clone(fun, index) {
                return;
            }
            let Some(f) = index.get(fun) else {
                return;
            };
            let formals = mono_icall_formals(f, args.len());
            let Some(key) = args_mono_key_idx(args, local_tys, funref_of, formals, index) else {
                return;
            };
            note_needed_clone(fun, key, f, index, needed);
        }
        // Parallel list HOFs pass FunRef callbacks as i64 ABI workers. Without
        // specializing `__lam_*` to Float, codegen emits Int `+` on IEEE bits.
        Value::Builtin {
            name: Builtin::ListParMap,
            args,
            ..
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
            ..
        } if args.len() == 3 => {
            let Some(cb) = funref_of.get(&args[2].0) else {
                return;
            };
            let Some(init_ty) = local_tys.get(&args[1].0) else {
                return;
            };
            // Prefer list elem; if list is still Int (If typed before arms),
            // Float init still forces Float/Float fold ABI.
            let elem = match local_tys.get(&args[0].0) {
                Some(Type::List(e)) => e.as_ref().clone(),
                _ if matches!(init_ty, Type::Float) => Type::Float,
                _ => return,
            };
            let Some(key) = types_mono_key(&[init_ty.clone(), elem]) else {
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

/// Formals for an IndirectCall: drop leading env when the callee is a closure
/// lam and the call site only passes user args.
pub(super) fn mono_icall_formals(f: &CoreFun, argc: usize) -> Option<&[Type]> {
    let ptys = f.param_tys.as_slice();
    if ptys.len() == argc {
        Some(ptys)
    } else if ptys.len() == argc + 1 && f.is_lifted_lambda() {
        Some(&ptys[1..])
    } else {
        Some(ptys)
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
    let ret = key.ret_ty(funs, Some(fun));
    if f.param_tys == param_tys && f.ret_ty == ret && key.funref_param_binds(&f.params).is_empty() {
        return;
    }
    needed.insert((fun.to_string(), key));
}

pub(crate) fn mono_value_ty(
    value: &Value,
    local_tys: &HashMap<u32, Type>,
    slot_tys: &HashMap<Sym, Type>,
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
    slot_tys: &HashMap<Sym, Type>,
    int_consts: &HashMap<u32, i64>,
    index: &FunIndex<'_>,
    funref_of: &HashMap<u32, String>,
) -> Type {
    let funs = index.funs();
    let mut call_ret = |fun: &str, args: &[Local]| -> Option<Type> {
        let formals = index
            .get(fun)
            .map(|f| mono_icall_formals(f, args.len()).unwrap_or(f.param_tys.as_slice()));
        // Prefer call-site mono key so `dbl(1.5)` types as Float before the
        // `dbl$Float` clone exists (ListAppend / fold otherwise keep List[Int]).
        if let Some(key) = args_mono_key_idx(args, local_tys, funref_of, formals, index) {
            if key.worth_cloning() || callee_is_mono_clone(fun, index) {
                let inferred = key.ret_ty(funs, Some(fun));
                if let Some(f) = index.get(fun) {
                    let ptys = materialize_mono_param_tys(&key, &f.param_tys, funs);
                    return Some(call_site_mono_ret(f, &inferred, &ptys, index));
                }
                return Some(inferred);
            }
        }
        if let Some(f) = index.get(fun) {
            return Some(f.ret_ty.clone());
        }
        if callee_is_mono_clone(fun, index) {
            if let Some(key) = args_mono_key_idx(args, local_tys, funref_of, None, index) {
                return Some(key.ret_ty(funs, Some(fun)));
            }
        }
        None
    };
    if let Value::IndirectCall { callee, args } = value {
        if let Some(name) = funref_of.get(&callee.0) {
            if let Some(t) = call_ret(name, args) {
                return t;
            }
        }
    }
    // FunRef / AllocClosure must see `__lam_*` rets even on the first let
    // (`spawn { Some(1.5) }` → Option[Float]). Do not seed all funs into
    // `fun_ret_tys` — Call prefers the table over call-site mono keys and
    // would erase List[Float]/Map rets (eps / idMap / fold tests).
    if let Value::FunRef(name) | Value::AllocClosure { fun: name, .. } = value {
        let f = index.get(name);
        let mut params = f.map(|f| f.param_tys.clone()).unwrap_or_default();
        let ret = f.map(|f| f.ret_ty.clone()).unwrap_or(Type::Int);
        if f.is_some_and(|f| f.is_lifted_lambda())
            && params
                .first()
                .is_some_and(|p| matches!(p, Type::Int | Type::Var(_)))
            && params.len() > 1
        {
            params.remove(0);
        }
        return Type::Fun(params, Box::new(ret), Effect::pure());
    }
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
            channel_elem_hint: index.channel_elem_hint,
            closure_cap_tys: None,
        },
        Some(&mut call_ret),
    )
}
