#![allow(clippy::too_many_arguments)]

use super::super::fun_index::FunIndex;
use super::super::key::{materialize_mono_param_tys, types_mono_key, MonoKey};
use crate::ir::{Block, CoreModule, Local, Op, Value};
use lumia_hir::Builtin;
use lumia_syntax::Sym;
use lumia_ty::Type;
use rustc_hash::FxHashMap as HashMap;

use super::collect::{
    args_mono_key_idx, callee_is_mono_clone, mono_icall_formals, mono_value_ty, note_scalar_consts,
};
use super::funref::{
    apply_funref_elem, chase_arm_funref, constant_adt_funref_ret_map, constant_funref_ret_map,
    constant_list_funref_ret_map, constant_returned_adt_funrefs, constant_returned_funref,
    constant_returned_list_funrefs, funref_elem_of_local, homogeneous_funref_elem,
    result_def_is_adt_field, FunrefElem, FunrefSlots,
};
use super::ret_refresh::call_site_mono_ret;

/// Rewrite every direct `Call(generic, …)` whose `(generic, key)` is in `renames`.
pub(super) fn rewrite_all_mono_call_sites(
    module: &mut CoreModule,
    renames: &HashMap<(Sym, MonoKey), Sym>,
) {
    // FunRef chase needs live bodies — snapshot before indexing signatures only.
    let join_funrefs = constant_funref_ret_map(&module.functions);
    let join_list_funrefs = constant_list_funref_ret_map(&module.functions);
    let join_adt_funrefs = constant_adt_funref_ret_map(&module.functions);
    let shadow = super::super::fun_index::SigShadow::from_module(module);
    let index = shadow.index();
    let no_funrefs = HashMap::default();
    let no_slot_funrefs = HashMap::default();
    for fun in &mut module.functions {
        let mut local_tys: HashMap<u32, Type> = HashMap::default();
        for (j, p) in fun.params.iter().enumerate() {
            local_tys.insert(p.0, fun.param_tys.get(j).cloned().unwrap_or(Type::Int));
        }
        let mut slot_tys: HashMap<Sym, Type> = HashMap::default();
        let mut int_consts: HashMap<u32, i64> = HashMap::default();
        let mut bool_consts: HashMap<u32, bool> = HashMap::default();
        let mut slot_list_funrefs: HashMap<Sym, FunrefSlots> = HashMap::default();
        let mut slot_adt_funrefs: HashMap<Sym, FunrefSlots> = HashMap::default();
        let mut list_funrefs: HashMap<u32, FunrefSlots> = HashMap::default();
        let mut adt_funrefs: HashMap<u32, FunrefSlots> = HashMap::default();
        let mut adt_tags: HashMap<u32, i64> = HashMap::default();
        rewrite_mono_block(
            &mut fun.body,
            &mut local_tys,
            &mut slot_tys,
            &mut int_consts,
            &mut bool_consts,
            &mut adt_tags,
            renames,
            &no_funrefs,
            &no_slot_funrefs,
            &mut slot_list_funrefs,
            &mut slot_adt_funrefs,
            &mut list_funrefs,
            &mut adt_funrefs,
            &index,
            &join_funrefs,
            &join_list_funrefs,
            &join_adt_funrefs,
        );
    }
}

fn rewrite_mono_block(
    block: &mut Block,
    local_tys: &mut HashMap<u32, Type>,
    slot_tys: &mut HashMap<Sym, Type>,
    int_consts: &mut HashMap<u32, i64>,
    bool_consts: &mut HashMap<u32, bool>,
    adt_tags: &mut HashMap<u32, i64>,
    renames: &HashMap<(Sym, MonoKey), Sym>,
    parent_funrefs: &HashMap<u32, Sym>,
    parent_slot_funrefs: &HashMap<Sym, Sym>,
    slot_list_funrefs: &mut HashMap<Sym, FunrefSlots>,
    slot_adt_funrefs: &mut HashMap<Sym, FunrefSlots>,
    list_funrefs: &mut HashMap<u32, FunrefSlots>,
    adt_funrefs: &mut HashMap<u32, FunrefSlots>,
    index: &FunIndex<'_>,
    join_funrefs: &HashMap<Sym, Sym>,
    join_list_funrefs: &HashMap<Sym, FunrefSlots>,
    join_adt_funrefs: &HashMap<Sym, FunrefSlots>,
) {
    let mut funref_of = parent_funrefs.clone();
    let mut slot_funrefs = parent_slot_funrefs.clone();
    let mut spawn_of: HashMap<u32, Sym> = HashMap::default();
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
                    bool_consts,
                    adt_tags,
                    renames,
                    &funref_of,
                    &slot_funrefs,
                    slot_list_funrefs,
                    slot_adt_funrefs,
                    list_funrefs,
                    adt_funrefs,
                    index,
                    join_funrefs,
                    join_list_funrefs,
                    join_adt_funrefs,
                );
                if let Some((cb_local, new_name)) = patch {
                    patch_funref_let(before, cb_local, &new_name);
                    funref_of.insert(cb_local, new_name.clone());
                }
                let ty = mono_value_ty_rewrite(
                    value, local_tys, slot_tys, int_consts, renames, &funref_of, index,
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
                    Some(join_funrefs),
                    Some(join_list_funrefs),
                    Some(join_adt_funrefs),
                    None,
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
        }
    }
}

fn par_hof_funref_patch(
    value: &Value,
    local_tys: &HashMap<u32, Type>,
    renames: &HashMap<(Sym, MonoKey), Sym>,
    funref_of: &HashMap<u32, Sym>,
) -> Option<(u32, Sym)> {
    match value {
        Value::Builtin {
            name: Builtin::ListParMap,
            args,
            ..
        } if args.len() == 2 => rewrite_par_hof_funref(
            args[1].0,
            &list_elem_ty(local_tys, args[0].0),
            renames,
            funref_of,
        ),
        Value::Builtin {
            name: Builtin::ListParFold,
            args,
            ..
        } if args.len() == 3 => {
            let mut tys = Vec::new();
            if let Some(t) = local_tys.get(&args[1].0) {
                tys.push(t.clone());
            }
            match local_tys.get(&args[0].0) {
                Some(Type::List(e)) => tys.push(e.as_ref().clone()),
                _ if tys.first().is_some_and(|t| matches!(t, Type::Float)) => {
                    tys.push(Type::Float);
                }
                _ => {}
            }
            rewrite_par_hof_funref(args[2].0, &tys, renames, funref_of)
        }
        _ => None,
    }
}

fn patch_funref_let(ops: &mut [Op], local: u32, new_name: &Sym) {
    for op in ops {
        if let Op::Let {
            local: l,
            value: Value::FunRef(n),
            ..
        } = op
        {
            if l.0 == local {
                *n = new_name.clone().into();
                return;
            }
        }
    }
}

fn rewrite_mono_value(
    value: &mut Value,
    local_tys: &mut HashMap<u32, Type>,
    slot_tys: &mut HashMap<Sym, Type>,
    int_consts: &mut HashMap<u32, i64>,
    bool_consts: &mut HashMap<u32, bool>,
    adt_tags: &mut HashMap<u32, i64>,
    renames: &HashMap<(Sym, MonoKey), Sym>,
    funref_of: &HashMap<u32, Sym>,
    slot_funrefs: &HashMap<Sym, Sym>,
    slot_list_funrefs: &mut HashMap<Sym, FunrefSlots>,
    slot_adt_funrefs: &mut HashMap<Sym, FunrefSlots>,
    list_funrefs: &mut HashMap<u32, FunrefSlots>,
    adt_funrefs: &mut HashMap<u32, FunrefSlots>,
    index: &FunIndex<'_>,
    join_funrefs: &HashMap<Sym, Sym>,
    join_list_funrefs: &HashMap<Sym, FunrefSlots>,
    join_adt_funrefs: &HashMap<Sym, FunrefSlots>,
) {
    match value {
        Value::Call { fun, args } => {
            if args.is_empty() || callee_is_mono_clone(fun.as_str(), index) {
                return;
            }
            let formals = index.get(fun.as_str()).map(|f| f.param_tys.as_slice());
            if let Some(key) = args_mono_key_idx(args, local_tys, funref_of, formals, index) {
                if let Some(new) = renames.get(&(fun.name.clone(), key)) {
                    *fun = new.clone().into();
                }
            }
        }
        Value::IndirectCall { callee, args } => {
            let Some(name) = funref_of.get(&callee.0).cloned() else {
                return;
            };
            if args.is_empty() || callee_is_mono_clone(&name, index) {
                return;
            }
            let formals = index
                .get(&name)
                .and_then(|f| mono_icall_formals(f, args.len()));
            if let Some(key) = args_mono_key_idx(args, local_tys, funref_of, formals, index) {
                if let Some(new) = renames.get(&(name.clone(), key)) {
                    *value = Value::Call {
                        fun: new.clone().into(),
                        args: args.clone(),
                    };
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
                    bool_consts,
                    adt_tags,
                    renames,
                    funref_of,
                    slot_funrefs,
                    slot_list_funrefs,
                    slot_adt_funrefs,
                    list_funrefs,
                    adt_funrefs,
                    index,
                    join_funrefs,
                    join_list_funrefs,
                    join_adt_funrefs,
                );
            });
        }
    }
}

/// Update funref / spawn / list-elem-funref / adt-field-funref maps after typing a `Let`.
pub(super) fn track_funref_after_let(
    local: u32,
    value: &Value,
    funref_of: &mut HashMap<u32, Sym>,
    spawn_of: &mut HashMap<u32, Sym>,
    list_funrefs: &mut HashMap<u32, FunrefSlots>,
    adt_funrefs: &mut HashMap<u32, FunrefSlots>,
    slot_funrefs: &HashMap<Sym, Sym>,
    slot_list_funrefs: &HashMap<Sym, FunrefSlots>,
    slot_adt_funrefs: &HashMap<Sym, FunrefSlots>,
    int_consts: &HashMap<u32, i64>,
    bool_consts: &HashMap<u32, bool>,
    join_funrefs: Option<&HashMap<Sym, Sym>>,
    join_list_funrefs: Option<&HashMap<Sym, FunrefSlots>>,
    join_adt_funrefs: Option<&HashMap<Sym, FunrefSlots>>,
    index: Option<&FunIndex<'_>>,
) {
    match value {
        Value::FunRef(name) => {
            funref_of.insert(local, name.name.clone());
            spawn_of.remove(&local);
            list_funrefs.remove(&local);
            adt_funrefs.remove(&local);
        }
        Value::AllocClosure { fun, .. } => {
            funref_of.insert(local, fun.name.clone());
            spawn_of.remove(&local);
            list_funrefs.remove(&local);
            adt_funrefs.remove(&local);
        }
        Value::Local(Local(src)) => {
            if let Some(n) = funref_of.get(src).cloned() {
                funref_of.insert(local, n);
            } else {
                funref_of.remove(&local);
            }
            if let Some(n) = spawn_of.get(src).cloned() {
                spawn_of.insert(local, n);
            } else {
                spawn_of.remove(&local);
            }
            if let Some(v) = list_funrefs.get(src).cloned() {
                list_funrefs.insert(local, v);
            } else {
                list_funrefs.remove(&local);
            }
            if let Some(v) = adt_funrefs.get(src).cloned() {
                adt_funrefs.insert(local, v);
            } else {
                adt_funrefs.remove(&local);
            }
        }
        Value::Name(n) => {
            if let Some(fr) = slot_funrefs.get(n).cloned() {
                funref_of.insert(local, fr);
            } else {
                funref_of.remove(&local);
            }
            spawn_of.remove(&local);
            if let Some(v) = slot_list_funrefs.get(n).cloned() {
                list_funrefs.insert(local, v);
            } else {
                list_funrefs.remove(&local);
            }
            if let Some(v) = slot_adt_funrefs.get(n).cloned() {
                adt_funrefs.insert(local, v);
            } else {
                adt_funrefs.remove(&local);
            }
        }
        Value::AllocList { elems, .. } => {
            let frs: FunrefSlots = elems
                .iter()
                .map(|e| funref_elem_of_local(e.0, funref_of, list_funrefs, adt_funrefs))
                .collect();
            if frs.iter().any(|x| x.is_some()) {
                list_funrefs.insert(local, frs);
            } else {
                list_funrefs.remove(&local);
            }
            funref_of.remove(&local);
            spawn_of.remove(&local);
            adt_funrefs.remove(&local);
        }
        Value::AllocAdt { fields, .. } => {
            let frs: FunrefSlots = fields
                .iter()
                .map(|e| funref_elem_of_local(e.0, funref_of, list_funrefs, adt_funrefs))
                .collect();
            if frs.iter().any(|x| x.is_some()) {
                adt_funrefs.insert(local, frs);
            } else {
                adt_funrefs.remove(&local);
            }
            funref_of.remove(&local);
            spawn_of.remove(&local);
            list_funrefs.remove(&local);
        }
        Value::Builtin {
            name: Builtin::ListGet,
            args,
            ..
        } if args.len() >= 2 => {
            let slots = list_funrefs.get(&args[0].0);
            let elem = int_consts
                .get(&args[1].0)
                .and_then(|idx| slots.and_then(|v| v.get(*idx as usize)).cloned().flatten())
                .or_else(|| slots.and_then(|v| homogeneous_funref_elem(v)));
            apply_funref_elem(local, elem, funref_of, list_funrefs, adt_funrefs);
            spawn_of.remove(&local);
        }
        Value::Builtin {
            name: Builtin::ListConcat,
            args,
            ..
        } if args.len() >= 2 => {
            // flatMap acc = ListConcat(acc, chunk): keep elem FunRefs so a later
            // ListGet can restore funref_of (Float ABI after icall).
            let left = list_funrefs.get(&args[0].0).cloned().unwrap_or_default();
            let right = list_funrefs.get(&args[1].0).cloned().unwrap_or_default();
            let mut frs = left;
            frs.extend(right);
            if frs.iter().any(|x| x.is_some()) {
                list_funrefs.insert(local, frs);
            } else {
                list_funrefs.remove(&local);
            }
            funref_of.remove(&local);
            spawn_of.remove(&local);
            adt_funrefs.remove(&local);
        }
        Value::Builtin {
            name: Builtin::ListAppend,
            args,
            ..
        } if args.len() >= 2 => {
            let mut frs = list_funrefs.get(&args[0].0).cloned().unwrap_or_default();
            frs.push(funref_elem_of_local(
                args[1].0,
                funref_of,
                list_funrefs,
                adt_funrefs,
            ));
            if frs.iter().any(|x| x.is_some()) {
                list_funrefs.insert(local, frs);
            } else {
                list_funrefs.remove(&local);
            }
            funref_of.remove(&local);
            spawn_of.remove(&local);
            adt_funrefs.remove(&local);
        }
        Value::Builtin {
            name:
                Builtin::ListTake
                | Builtin::ListReverse
                | Builtin::ListSlice
                | Builtin::ListParMap
                | Builtin::Elems,
            args,
            ..
        } if !args.is_empty() => {
            // Preserve Fun elem refs through identity-ish list transforms.
            if let Some(v) = list_funrefs.get(&args[0].0).cloned() {
                list_funrefs.insert(local, v);
            } else {
                list_funrefs.remove(&local);
            }
            funref_of.remove(&local);
            spawn_of.remove(&local);
            adt_funrefs.remove(&local);
        }
        Value::Builtin {
            name: Builtin::AdtField,
            args,
            ..
        } if !args.is_empty() => {
            // Product field holding a Fun / nested container.
            let indexed = args.get(1).and_then(|idx| {
                int_consts.get(&idx.0).and_then(|i| {
                    if *i < 0 {
                        return None;
                    }
                    adt_funrefs
                        .get(&args[0].0)
                        .and_then(|v| v.get(*i as usize))
                        .cloned()
                        .flatten()
                })
            });
            let elem = indexed.or_else(|| funref_of.get(&args[0].0).cloned().map(FunrefElem::Fun));
            apply_funref_elem(local, elem, funref_of, list_funrefs, adt_funrefs);
            spawn_of.remove(&local);
        }
        Value::If {
            cond,
            then_block,
            else_block,
            ..
        } => {
            let then_fr = then_block.result.and_then(|Local(r)| {
                chase_arm_funref(then_block, r, funref_of, adt_funrefs, int_consts)
            });
            let else_fr = else_block.result.and_then(|Local(r)| {
                chase_arm_funref(else_block, r, funref_of, adt_funrefs, int_consts)
            });
            match (then_fr, else_fr) {
                (Some(a), Some(b)) if a == b => {
                    funref_of.insert(local, a);
                }
                (Some(a), Some(b)) => {
                    // Distinct arms (e.g. `Some(f) alt g`): only bind when the
                    // condition is a known constant so we do not Call the wrong lam.
                    match bool_consts.get(&cond.0).copied() {
                        Some(true) => {
                            funref_of.insert(local, a);
                        }
                        Some(false) => {
                            funref_of.insert(local, b);
                        }
                        None => {
                            funref_of.remove(&local);
                        }
                    }
                }
                // Do not bind a single arm when the other is an AdtField payload
                // extract (`Some(f) alt g` must not become `g` when then chase fails).
                (Some(a), None) if !result_def_is_adt_field(else_block) => {
                    funref_of.insert(local, a);
                }
                (None, Some(a)) if !result_def_is_adt_field(then_block) => {
                    funref_of.insert(local, a);
                }
                _ => {
                    funref_of.remove(&local);
                }
            }
            spawn_of.remove(&local);
            list_funrefs.remove(&local);
            adt_funrefs.remove(&local);
        }
        Value::Call { fun, args } => {
            // `unwrapOr(opt, default)`: propagate Fun from Some/Ok field0 or default.
            let base = index
                .and_then(|ix| ix.get(fun.as_str()))
                .map(|f| f.base_name())
                .unwrap_or_else(|| super::super::key::strip_mono_suffix(fun.as_str()));
            if base == "unwrapOr" && args.len() >= 2 {
                let from_opt = adt_funrefs
                    .get(&args[0].0)
                    .and_then(|v| v.first())
                    .cloned()
                    .flatten();
                let from_default =
                    funref_elem_of_local(args[1].0, funref_of, list_funrefs, adt_funrefs);
                // Prefer payload Fun when present; else default (None/Err path).
                let elem = match (&from_opt, &from_default) {
                    (Some(FunrefElem::Fun(_)), _) => from_opt,
                    (None, Some(_)) => from_default,
                    (Some(_), _) => from_opt,
                    _ => from_default,
                };
                apply_funref_elem(local, elem, funref_of, list_funrefs, adt_funrefs);
            } else {
                funref_of.remove(&local);
                list_funrefs.remove(&local);
                adt_funrefs.remove(&local);
            }
            spawn_of.remove(&local);
        }
        Value::Builtin {
            name: Builtin::TaskSpawn,
            args,
            ..
        } if args.len() == 1 => {
            if let Some(n) = funref_of.get(&args[0].0).cloned() {
                spawn_of.insert(local, n);
            } else {
                spawn_of.remove(&local);
            }
            funref_of.remove(&local);
            list_funrefs.remove(&local);
            adt_funrefs.remove(&local);
        }
        Value::Builtin {
            name: Builtin::TaskJoin,
            args,
            ..
        } if args.len() == 1 => {
            let spawned = spawn_of.get(&args[0].0).cloned();
            let joined = spawned.as_ref().and_then(|s| {
                join_funrefs
                    .and_then(|m| m.get(s.as_str()).cloned())
                    .or_else(|| index.and_then(|idx| constant_returned_funref(s.as_str(), idx)))
            });
            if let Some(inner) = joined {
                funref_of.insert(local, inner);
            } else {
                funref_of.remove(&local);
            }
            let list = spawned.as_ref().and_then(|s| {
                join_list_funrefs
                    .and_then(|m| m.get(s.as_str()).cloned())
                    .or_else(|| index.and_then(|idx| constant_returned_list_funrefs(s.as_str(), idx)))
            });
            if let Some(v) = list {
                list_funrefs.insert(local, v);
            } else {
                list_funrefs.remove(&local);
            }
            let adt = spawned.as_ref().and_then(|s| {
                join_adt_funrefs
                    .and_then(|m| m.get(s.as_str()).cloned())
                    .or_else(|| index.and_then(|idx| constant_returned_adt_funrefs(s.as_str(), idx)))
            });
            if let Some(v) = adt {
                adt_funrefs.insert(local, v);
            } else {
                adt_funrefs.remove(&local);
            }
            spawn_of.remove(&local);
        }
        _ => {
            funref_of.remove(&local);
            spawn_of.remove(&local);
            list_funrefs.remove(&local);
            adt_funrefs.remove(&local);
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
    renames: &HashMap<(Sym, MonoKey), Sym>,
    funref_of: &HashMap<u32, Sym>,
) -> Option<(u32, Sym)> {
    let cb = funref_of.get(&cb_local)?;
    let key = types_mono_key(cb_param_tys)?;
    let new = renames.get(&(cb.clone(), key))?;
    Some((cb_local, new.clone()))
}

fn mono_value_ty_rewrite(
    value: &Value,
    local_tys: &HashMap<u32, Type>,
    slot_tys: &HashMap<Sym, Type>,
    int_consts: &HashMap<u32, i64>,
    renames: &HashMap<(Sym, MonoKey), Sym>,
    funref_of: &HashMap<u32, Sym>,
    index: &FunIndex<'_>,
) -> Type {
    let funs = index.funs();
    match value {
        Value::Call { fun, args } => {
            // Must match scan's `call_site_mono_ret`: bare `MonoKey::ret_ty` uses
            // "last data arg", so `l2Normalize(list, eps)` becomes `Float` and
            // poisons `var u` → later `nAddmm` keys miss `List_Float` clones.
            let formals = index.get(fun.as_str()).map(|f| f.param_tys.as_slice());
            if let Some(key) = args_mono_key_idx(args, local_tys, funref_of, formals, index) {
                let inferred = key.ret_ty(funs, Some(fun.as_str()));
                let renamed = renames.get(&(fun.name.clone(), key.clone()));
                let already_clone = renames.iter().any(|(_, n)| n.as_str() == fun.as_str())
                    || callee_is_mono_clone(fun.as_str(), index);
                if renamed.is_some() || already_clone || key.worth_cloning() {
                    let callee = renamed.map(|s| s.as_str()).unwrap_or(fun.as_str());
                    if let Some(f) = index.get(callee).or_else(|| index.get(fun.as_str())) {
                        let ptys = materialize_mono_param_tys(&key, &f.param_tys, funs);
                        return call_site_mono_ret(f, &inferred, &ptys, index);
                    }
                    return inferred;
                }
            } else if let Some(((_, mk), _)) =
                renames.iter().find(|(_, n)| n.as_str() == fun.as_str())
            {
                let inferred = mk.ret_ty(funs, Some(fun.as_str()));
                if let Some(f) = index.get(fun.as_str()) {
                    let ptys = materialize_mono_param_tys(mk, &f.param_tys, funs);
                    return call_site_mono_ret(f, &inferred, &ptys, index);
                }
                return inferred;
            }
            if let Some(f) = index.get(fun.as_str()) {
                return f.ret_ty.clone();
            }
            Type::Int
        }
        other => mono_value_ty(other, local_tys, slot_tys, int_consts, index),
    }
}

#[cfg(test)]
#[path = "../tests/specialize_rewrite_near.rs"]
mod tests;
