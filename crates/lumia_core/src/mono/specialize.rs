use super::fun_index::FunIndex;
use super::key::{
    args_mono_key, ground_open_vars, materialize_mono_param_tys, types_mono_key, MonoKey, MonoKind,
};
use super::ret_ty::{block_result_fixed_ty, param_ty_map, refine_mono_container_ret};
use super::traits::directize_block;
use crate::ir::{Block, CoreFun, CoreModule, Local, Op, Value};
use crate::value_ty::{infer_value_ty_ctx, InferValueCtx};
use lumia_hir::Builtin;
use lumia_ty::{Effect, Type};
use rustc_hash::{FxHashMap as HashMap, FxHashSet};

/// Nested Fun-in-container snapshot for ListGet / AdtField / TaskJoin.
#[derive(Clone, Debug, PartialEq, Eq)]
enum FunrefElem {
    Fun(String),
    List(Vec<Option<FunrefElem>>),
    Adt(Vec<Option<FunrefElem>>),
}

type FunrefSlots = Vec<Option<FunrefElem>>;

fn funref_elem_of_local(
    id: u32,
    funref_of: &HashMap<u32, String>,
    list_funrefs: &HashMap<u32, FunrefSlots>,
    adt_funrefs: &HashMap<u32, FunrefSlots>,
) -> Option<FunrefElem> {
    if let Some(n) = funref_of.get(&id) {
        return Some(FunrefElem::Fun(n.clone()));
    }
    if let Some(v) = list_funrefs.get(&id) {
        return Some(FunrefElem::List(v.clone()));
    }
    if let Some(v) = adt_funrefs.get(&id) {
        return Some(FunrefElem::Adt(v.clone()));
    }
    None
}

fn apply_funref_elem(
    local: u32,
    elem: Option<FunrefElem>,
    funref_of: &mut HashMap<u32, String>,
    list_funrefs: &mut HashMap<u32, FunrefSlots>,
    adt_funrefs: &mut HashMap<u32, FunrefSlots>,
) {
    match elem {
        Some(FunrefElem::Fun(n)) => {
            funref_of.insert(local, n);
            list_funrefs.remove(&local);
            adt_funrefs.remove(&local);
        }
        Some(FunrefElem::List(v)) => {
            if v.iter().any(|x| x.is_some()) {
                list_funrefs.insert(local, v);
            } else {
                list_funrefs.remove(&local);
            }
            funref_of.remove(&local);
            adt_funrefs.remove(&local);
        }
        Some(FunrefElem::Adt(v)) => {
            if v.iter().any(|x| x.is_some()) {
                adt_funrefs.insert(local, v);
            } else {
                adt_funrefs.remove(&local);
            }
            funref_of.remove(&local);
            list_funrefs.remove(&local);
        }
        None => {
            funref_of.remove(&local);
            list_funrefs.remove(&local);
            adt_funrefs.remove(&local);
        }
    }
}

/// When `ListGet` index is not a const (filter loop), recover a Fun if every
/// present slot names the same function / nested shape.
fn homogeneous_funref_elem(slots: &FunrefSlots) -> Option<FunrefElem> {
    let mut found: Option<&FunrefElem> = None;
    for s in slots {
        let Some(e) = s else {
            continue;
        };
        match found {
            None => found = Some(e),
            Some(prev) if prev == e => {}
            _ => return None,
        }
    }
    found.cloned()
}

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
pub(crate) fn specialize_mono_calls(module: &mut CoreModule) -> bool {
    let renames = collect_mono_clones_until_fixed_point(module);
    if renames.is_empty() {
        return false;
    }
    rewrite_all_mono_call_sites(module, &renames);
    // Residual `Call(generic, …)` (missed rewrite) must not emit Int `*` on IEEE
    // bits — upgrade erased formals from Float/List[Float] clones.
    upgrade_generic_param_tys_from_clones(module);
    // After all clones exist, upgrade erased Int rets on HOF wrappers whose
    // bodies now `Call(dbl$Float, …)` (directize order within a round varies).
    refresh_erased_mono_return_types(module);
    // Toehold: thin FunRef wrappers that only forward to a concrete Call share
    // that target at call sites (avoid an extra frame / duplicate body emit).
    elide_trivial_mono_forwarders(module);
    true
}

/// When a mono clone has Float / List[Float] / … formals but the generic still
/// carries ABI `Int` / `List[Int]`, copy the clone's ground types onto the
/// generic. Missed call-site rewrites then still get correct float arith in
/// codegen (instead of `smul` on IEEE bits → `lumia_trap_overflow`).
fn upgrade_generic_param_tys_from_clones(module: &mut CoreModule) {
    let upgrades: Vec<(String, Vec<Type>, Type)> = {
        let mut best: HashMap<String, (Vec<Type>, Type)> = HashMap::default();
        for f in &module.functions {
            let Some(orig) = f.mono_of.as_ref() else {
                continue;
            };
            let entry = best.entry(orig.clone()).or_insert_with(|| (f.param_tys.clone(), f.ret_ty.clone()));
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
    // Bodies are moved out so FunIndex can borrow signatures immutably — but
    // `TaskJoin` FunRef chase needs those bodies. Snapshot constant FunRef rets
    // before emptying.
    let join_funrefs = constant_funref_ret_map(&module.functions);
    let join_list_funrefs = constant_list_funref_ret_map(&module.functions);
    let join_adt_funrefs = constant_adt_funref_ret_map(&module.functions);
    let mut functions = std::mem::take(&mut module.functions);
    let empty = Block {
        ops: vec![],
        result: None,
    };
    let mut bodies: Vec<Block> = functions
        .iter_mut()
        .map(|f| std::mem::replace(&mut f.body, empty.clone()))
        .collect();
    {
        let index = FunIndex::new(&functions, &module.sum_max_arity, &module.trait_methods, module.channel_elem_hint.as_ref());
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
            let mut bool_consts: HashMap<u32, bool> = HashMap::default();
            let mut slot_list_funrefs: HashMap<String, FunrefSlots> = HashMap::default();
            let mut slot_adt_funrefs: HashMap<String, FunrefSlots> = HashMap::default();
            let mut list_funrefs: HashMap<u32, FunrefSlots> = HashMap::default();
            let mut adt_funrefs: HashMap<u32, FunrefSlots> = HashMap::default();
            let mut adt_tags: HashMap<u32, i64> = HashMap::default();
            rewrite_mono_block(
                &mut bodies[i],
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
    for (fun, body) in functions.iter_mut().zip(bodies) {
        fun.body = body;
    }
    module.functions = functions;
}

fn refresh_erased_mono_return_types(module: &mut CoreModule) {
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

/// One scan→clone pass. Returns true if any new clone was appended.
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
        let mut slot_tys: HashMap<String, Type> = HashMap::default();
        let mut int_consts: HashMap<u32, i64> = HashMap::default();
        // Shared across nested Loop/If like `slot_tys` so flatMap's mut acc
        // (`ListConcat` in the loop body) is visible to post-loop `ListGet`.
        let mut slot_list_funrefs: HashMap<String, FunrefSlots> = HashMap::default();
        let mut slot_adt_funrefs: HashMap<String, FunrefSlots> = HashMap::default();
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
        let ret_ty = mono_clone_ret_ty(&clone, &inferred, &index);
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
fn mono_clone_ret_ty(fun: &CoreFun, inferred: &Type, index: &FunIndex<'_>) -> Type {
    let param_map = param_ty_map(fun);
    let raw = if let Some(t) =
        block_result_fixed_ty(&fun.body, index, index.trait_methods, &param_map)
    {
        // Nested `andThen` bodies often join to `Option[Option[Int]]` / `Option[Int]`
        // while the FunRef key already knows `Option[Float]` — prefer the key.
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
    // Open Vars survive refine when Err/elem slots stay polymorphic — then
    // `type_to_mono` fails and follow-on `unwrapOr` never clones.
    ground_open_vars(raw)
}

/// When body typing lags behind the mono key (erased Int / nested Option), take
/// the inferred payload — fixes `andThen(…, { x -> andThen(…) })` then unwrapOr.
fn merge_mono_ret_with_inferred(body: Type, inferred: &Type) -> Type {
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
        ) if bn == inan && (bn == "Option" || bn == "Result") => {
            let body_payload = bp.first();
            let inf_payload = ip.first();
            if option_result_payload_weaker(body_payload, inf_payload) {
                return inferred.clone();
            }
            refine_mono_container_ret(&body, inferred)
        }
        (Type::Int | Type::Var(_), _) => match inferred {
            // Scalar upgrades from erased Int (Float ABI, bool, …).
            Type::Float
            | Type::Bool
            | Type::String
            | Type::Char
            | Type::Fun(_, _, _) => inferred.clone(),
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
        Some(Type::Adt { name, params }) if name == "Option" || name == "Result" => {
            params
                .first()
                .is_none_or(|p| matches!(p, Type::Int | Type::Var(_)))
                || !matches!(inf, Type::Adt { name: n, .. } if n == "Option" || n == "Result")
        }
        _ => false,
    }
}

/// Call-site ret while scanning/rewriting: same body-first strategy as
/// [`mono_clone_ret_ty`]. With call-site formals, `touch(b, eps)` resolves to
/// the product (not MonoKey's trailing `Float`), so later `addx` keys match.
fn call_site_mono_ret(
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
    let raw = if let Some(t) =
        block_result_fixed_ty(&fun.body, index, index.trait_methods, &param_map)
    {
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

fn scan_mono_block(
    block: &Block,
    local_tys: &mut HashMap<u32, Type>,
    slot_tys: &mut HashMap<String, Type>,
    int_consts: &mut HashMap<u32, i64>,
    bool_consts: &mut HashMap<u32, bool>,
    index: &FunIndex<'_>,
    needed: &mut FxHashSet<(String, MonoKey)>,
    parent_funrefs: &HashMap<u32, String>,
    parent_slot_funrefs: &HashMap<String, String>,
    slot_list_funrefs: &mut HashMap<String, FunrefSlots>,
    slot_adt_funrefs: &mut HashMap<String, FunrefSlots>,
    list_funrefs: &mut HashMap<u32, FunrefSlots>,
    adt_funrefs: &mut HashMap<u32, FunrefSlots>,
    adt_tags: &mut HashMap<u32, i64>,
) {
    let mut funref_of = parent_funrefs.clone();
    let mut slot_funrefs = parent_slot_funrefs.clone();
    // Task local → spawned FunRef/AllocClosure name (for join → FunRef chase).
    let mut spawn_of: HashMap<u32, String> = HashMap::default();
    for op in &block.ops {
        match op {
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
        }
    }
}

fn note_scalar_consts(
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
            op: lumia_syntax::BinOp::Eq,
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
            op: lumia_syntax::BinOp::Ne,
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

/// If `fun`'s result is a constant `FunRef` / `AllocClosure`, return that name.
fn constant_returned_funref(fun: &str, index: &FunIndex<'_>) -> Option<String> {
    let f = index.get(fun)?;
    constant_returned_funref_in_body(&f.body)
}

fn constant_returned_funref_in_body(body: &Block) -> Option<String> {
    let Local(mut cur) = body.result?;
    let mut seen = FxHashSet::default();
    loop {
        if !seen.insert(cur) {
            return None;
        }
        let mut found: Option<&Value> = None;
        for op in &body.ops {
            if let Op::Let { local, value, .. } = op {
                if local.0 == cur {
                    found = Some(value);
                    break;
                }
            }
        }
        match found? {
            Value::Local(Local(src)) => cur = *src,
            Value::FunRef(n) | Value::AllocClosure { fun: n, .. } => return Some(n.clone()),
            _ => return None,
        }
    }
}

/// Snapshot before rewrite empties bodies (FunIndex then has empty `body`s).
fn constant_funref_ret_map(functions: &[CoreFun]) -> HashMap<String, String> {
    let mut out = HashMap::default();
    for f in functions {
        if let Some(n) = constant_returned_funref_in_body(&f.body) {
            out.insert(f.name.clone(), n);
        }
    }
    out
}

/// Spawn bodies that return `listOf(fun, …)` — elem FunRefs for join→ListGet.
fn constant_list_funref_ret_map(
    functions: &[CoreFun],
) -> HashMap<String, FunrefSlots> {
    let mut out = HashMap::default();
    for f in functions {
        if let Some(v) = constant_returned_list_funrefs_in_body(&f.body) {
            out.insert(f.name.clone(), v);
        }
    }
    out
}

/// Spawn bodies that return `Box { f = fun, … }` — field FunRefs for join→AdtField.
fn constant_adt_funref_ret_map(
    functions: &[CoreFun],
) -> HashMap<String, FunrefSlots> {
    let mut out = HashMap::default();
    for f in functions {
        if let Some(v) = constant_returned_adt_funrefs_in_body(&f.body) {
            out.insert(f.name.clone(), v);
        }
    }
    out
}

fn constant_returned_list_funrefs(fun: &str, index: &FunIndex<'_>) -> Option<FunrefSlots> {
    let f = index.get(fun)?;
    constant_returned_list_funrefs_in_body(&f.body)
}

fn constant_returned_list_funrefs_in_body(body: &Block) -> Option<FunrefSlots> {
    let Local(mut cur) = body.result?;
    let mut seen = FxHashSet::default();
    loop {
        if !seen.insert(cur) {
            return None;
        }
        let found = def_of(body, cur)?;
        match found {
            Value::Local(Local(src)) => cur = *src,
            Value::AllocList { elems, .. } => {
                let frs: FunrefSlots = elems
                    .iter()
                    .map(|e| chase_local_funref_elem(body, e.0))
                    .collect();
                if frs.iter().any(|x| x.is_some()) {
                    return Some(frs);
                }
                return None;
            }
            _ => return None,
        }
    }
}

fn constant_returned_adt_funrefs(fun: &str, index: &FunIndex<'_>) -> Option<FunrefSlots> {
    let f = index.get(fun)?;
    constant_returned_adt_funrefs_in_body(&f.body)
}

fn constant_returned_adt_funrefs_in_body(body: &Block) -> Option<FunrefSlots> {
    let Local(mut cur) = body.result?;
    let mut seen = FxHashSet::default();
    loop {
        if !seen.insert(cur) {
            return None;
        }
        let found = def_of(body, cur)?;
        match found {
            Value::Local(Local(src)) => cur = *src,
            Value::AllocAdt { fields, .. } => {
                let frs: FunrefSlots = fields
                    .iter()
                    .map(|e| chase_local_funref_elem(body, e.0))
                    .collect();
                if frs.iter().any(|x| x.is_some()) {
                    return Some(frs);
                }
                return None;
            }
            _ => return None,
        }
    }
}

fn def_of(body: &Block, id: u32) -> Option<&Value> {
    for op in &body.ops {
        if let Op::Let { local, value, .. } = op {
            if local.0 == id {
                return Some(value);
            }
        }
    }
    None
}

fn chase_local_funref(body: &Block, id: u32) -> Option<String> {
    match chase_local_funref_elem(body, id)? {
        FunrefElem::Fun(n) => Some(n),
        _ => None,
    }
}

/// Chase Fun / nested List / nested Adt funrefs inside a single function body.
fn chase_local_funref_elem(body: &Block, id: u32) -> Option<FunrefElem> {
    let mut cur = id;
    let mut seen = FxHashSet::default();
    for _ in 0..24 {
        if !seen.insert(cur) {
            return None;
        }
        match def_of(body, cur)? {
            Value::Local(Local(src)) => cur = *src,
            Value::FunRef(n) | Value::AllocClosure { fun: n, .. } => {
                return Some(FunrefElem::Fun(n.clone()));
            }
            Value::AllocList { elems, .. } => {
                let frs: FunrefSlots = elems
                    .iter()
                    .map(|e| chase_local_funref_elem(body, e.0))
                    .collect();
                if frs.iter().any(|x| x.is_some()) {
                    return Some(FunrefElem::List(frs));
                }
                return None;
            }
            Value::AllocAdt { fields, .. } => {
                let frs: FunrefSlots = fields
                    .iter()
                    .map(|e| chase_local_funref_elem(body, e.0))
                    .collect();
                if frs.iter().any(|x| x.is_some()) {
                    return Some(FunrefElem::Adt(frs));
                }
                return None;
            }
            _ => return None,
        }
    }
    None
}

fn result_def_is_adt_field(body: &Block) -> bool {
    let Some(Local(mut cur)) = body.result else {
        return false;
    };
    let mut seen = FxHashSet::default();
    for _ in 0..8 {
        if !seen.insert(cur) {
            return false;
        }
        match def_of(body, cur) {
            Some(Value::Local(Local(src))) => cur = *src,
            Some(Value::Builtin {
                name: Builtin::AdtField,
                ..
            }) => return true,
            _ => return false,
        }
    }
    false
}

/// Resolve funref for an If arm result, including `AdtField` of a known ADT.
fn chase_arm_funref(
    body: &Block,
    id: u32,
    funref_of: &HashMap<u32, String>,
    adt_funrefs: &HashMap<u32, FunrefSlots>,
    int_consts: &HashMap<u32, i64>,
) -> Option<String> {
    let mut cur = id;
    let mut seen = FxHashSet::default();
    for _ in 0..16 {
        if !seen.insert(cur) {
            return None;
        }
        if let Some(n) = funref_of.get(&cur) {
            return Some(n.clone());
        }
        match def_of(body, cur) {
            Some(Value::Local(Local(src))) => cur = *src,
            Some(Value::FunRef(n) | Value::AllocClosure { fun: n, .. }) => {
                return Some(n.clone());
            }
            Some(Value::Builtin {
                name: Builtin::AdtField,
                args,
                ..
            }) if args.len() >= 2 => {
                let idx = int_consts.get(&args[1].0).copied().unwrap_or(-1);
                if idx < 0 {
                    return None;
                }
                return adt_funrefs
                    .get(&args[0].0)
                    .and_then(|v| v.get(idx as usize))
                    .and_then(|o| match o {
                        Some(FunrefElem::Fun(n)) => Some(n.clone()),
                        _ => None,
                    });
            }
            _ => return chase_local_funref(body, cur),
        }
    }
    None
}

fn walk_mono_nested_scan(
    value: &Value,
    local_tys: &mut HashMap<u32, Type>,
    slot_tys: &mut HashMap<String, Type>,
    int_consts: &mut HashMap<u32, i64>,
    bool_consts: &mut HashMap<u32, bool>,
    index: &FunIndex<'_>,
    needed: &mut FxHashSet<(String, MonoKey)>,
    funref_of: &HashMap<u32, String>,
    slot_funrefs: &HashMap<String, String>,
    slot_list_funrefs: &mut HashMap<String, FunrefSlots>,
    slot_adt_funrefs: &mut HashMap<String, FunrefSlots>,
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
            let Some(key) = args_mono_key(args, local_tys, funref_of, formals) else {
                return;
            };
            note_needed_clone(fun, key, f, index, needed);
        }
        // Parallel list HOFs pass FunRef callbacks as i64 ABI workers. Without
        // specializing `__lam_*` to Float, codegen emits Int `+` on IEEE bits.
        Value::Builtin {
            name: Builtin::ListParMap,
            args, .. } if args.len() == 2 => {
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
            args, .. } if args.len() == 3 => {
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
fn mono_icall_formals(f: &CoreFun, argc: usize) -> Option<&[Type]> {
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
        let formals = index.get(fun).map(|f| {
            mono_icall_formals(f, args.len()).unwrap_or(f.param_tys.as_slice())
        });
        // Prefer call-site mono key so `dbl(1.5)` types as Float before the
        // `dbl$Float` clone exists (ListAppend / fold otherwise keep List[Int]).
        if let Some(key) = args_mono_key(args, local_tys, funref_of, formals) {
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
            if let Some(key) = args_mono_key(args, local_tys, funref_of, None) {
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
        },
        Some(&mut call_ret),
    )
}

fn rewrite_mono_block(
    block: &mut Block,
    local_tys: &mut HashMap<u32, Type>,
    slot_tys: &mut HashMap<String, Type>,
    int_consts: &mut HashMap<u32, i64>,
    bool_consts: &mut HashMap<u32, bool>,
    adt_tags: &mut HashMap<u32, i64>,
    renames: &HashMap<(String, MonoKey), String>,
    parent_funrefs: &HashMap<u32, String>,
    parent_slot_funrefs: &HashMap<String, String>,
    slot_list_funrefs: &mut HashMap<String, FunrefSlots>,
    slot_adt_funrefs: &mut HashMap<String, FunrefSlots>,
    list_funrefs: &mut HashMap<u32, FunrefSlots>,
    adt_funrefs: &mut HashMap<u32, FunrefSlots>,
    index: &FunIndex<'_>,
    join_funrefs: &HashMap<String, String>,
    join_list_funrefs: &HashMap<String, FunrefSlots>,
    join_adt_funrefs: &HashMap<String, FunrefSlots>,
) {
    let mut funref_of = parent_funrefs.clone();
    let mut slot_funrefs = parent_slot_funrefs.clone();
    let mut spawn_of: HashMap<u32, String> = HashMap::default();
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
                    funref_of.insert(cb_local, new_name);
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
    renames: &HashMap<(String, MonoKey), String>,
    funref_of: &HashMap<u32, String>,
) -> Option<(u32, String)> {
    match value {
        Value::Builtin {
            name: Builtin::ListParMap,
            args, .. } if args.len() == 2 => rewrite_par_hof_funref(
            args[1].0,
            &list_elem_ty(local_tys, args[0].0),
            renames,
            funref_of,
        ),
        Value::Builtin {
            name: Builtin::ListParFold,
            args, .. } if args.len() == 3 => {
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
    bool_consts: &mut HashMap<u32, bool>,
    adt_tags: &mut HashMap<u32, i64>,
    renames: &HashMap<(String, MonoKey), String>,
    funref_of: &HashMap<u32, String>,
    slot_funrefs: &HashMap<String, String>,
    slot_list_funrefs: &mut HashMap<String, FunrefSlots>,
    slot_adt_funrefs: &mut HashMap<String, FunrefSlots>,
    list_funrefs: &mut HashMap<u32, FunrefSlots>,
    adt_funrefs: &mut HashMap<u32, FunrefSlots>,
    index: &FunIndex<'_>,
    join_funrefs: &HashMap<String, String>,
    join_list_funrefs: &HashMap<String, FunrefSlots>,
    join_adt_funrefs: &HashMap<String, FunrefSlots>,
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
            if let Some(key) = args_mono_key(args, local_tys, funref_of, formals) {
                if let Some(new) = renames.get(&(name, key)) {
                    *value = Value::Call {
                        fun: new.clone(),
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
fn track_funref_after_let(
    local: u32,
    value: &Value,
    funref_of: &mut HashMap<u32, String>,
    spawn_of: &mut HashMap<u32, String>,
    list_funrefs: &mut HashMap<u32, FunrefSlots>,
    adt_funrefs: &mut HashMap<u32, FunrefSlots>,
    slot_funrefs: &HashMap<String, String>,
    slot_list_funrefs: &HashMap<String, FunrefSlots>,
    slot_adt_funrefs: &HashMap<String, FunrefSlots>,
    int_consts: &HashMap<u32, i64>,
    bool_consts: &HashMap<u32, bool>,
    join_funrefs: Option<&HashMap<String, String>>,
    join_list_funrefs: Option<&HashMap<String, FunrefSlots>>,
    join_adt_funrefs: Option<&HashMap<String, FunrefSlots>>,
    index: Option<&FunIndex<'_>>,
) {
    match value {
        Value::FunRef(name) => {
            funref_of.insert(local, name.clone());
            spawn_of.remove(&local);
            list_funrefs.remove(&local);
            adt_funrefs.remove(&local);
        }
        Value::AllocClosure { fun, .. } => {
            funref_of.insert(local, fun.clone());
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
                .and_then(|idx| {
                    slots
                        .and_then(|v| v.get(*idx as usize))
                        .cloned()
                        .flatten()
                })
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
            let left = list_funrefs
                .get(&args[0].0)
                .cloned()
                .unwrap_or_default();
            let right = list_funrefs
                .get(&args[1].0)
                .cloned()
                .unwrap_or_default();
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
            let mut frs = list_funrefs
                .get(&args[0].0)
                .cloned()
                .unwrap_or_default();
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
            name: Builtin::ListTake
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
            let elem = indexed.or_else(|| {
                funref_of
                    .get(&args[0].0)
                    .cloned()
                    .map(FunrefElem::Fun)
            });
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
                .and_then(|ix| ix.get(fun))
                .map(|f| f.base_name())
                .unwrap_or_else(|| fun.split('$').next().unwrap_or(fun.as_str()));
            if base == "unwrapOr" && args.len() >= 2 {
                let from_opt = adt_funrefs
                    .get(&args[0].0)
                    .and_then(|v| v.first())
                    .cloned()
                    .flatten();
                let from_default = funref_elem_of_local(
                    args[1].0,
                    funref_of,
                    list_funrefs,
                    adt_funrefs,
                );
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
                    .and_then(|m| m.get(s).cloned())
                    .or_else(|| {
                        index.and_then(|idx| constant_returned_funref(s, idx))
                    })
            });
            if let Some(inner) = joined {
                funref_of.insert(local, inner);
            } else {
                funref_of.remove(&local);
            }
            let list = spawned.as_ref().and_then(|s| {
                join_list_funrefs
                    .and_then(|m| m.get(s).cloned())
                    .or_else(|| {
                        index.and_then(|idx| constant_returned_list_funrefs(s, idx))
                    })
            });
            if let Some(v) = list {
                list_funrefs.insert(local, v);
            } else {
                list_funrefs.remove(&local);
            }
            let adt = spawned.as_ref().and_then(|s| {
                join_adt_funrefs
                    .and_then(|m| m.get(s).cloned())
                    .or_else(|| {
                        index.and_then(|idx| constant_returned_adt_funrefs(s, idx))
                    })
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
            // Must match scan's `call_site_mono_ret`: bare `MonoKey::ret_ty` uses
            // "last data arg", so `l2Normalize(list, eps)` becomes `Float` and
            // poisons `var u` → later `nAddmm` keys miss `List_Float` clones.
            let formals = index.get(fun).map(|f| f.param_tys.as_slice());
            if let Some(key) = args_mono_key(args, local_tys, funref_of, formals) {
                let inferred = key.ret_ty(funs, Some(fun.as_str()));
                let renamed = renames.get(&(fun.clone(), key.clone()));
                let already_clone =
                    renames.iter().any(|(_, n)| n == fun) || callee_is_mono_clone(fun, index);
                if renamed.is_some() || already_clone || key.worth_cloning() {
                    let callee = renamed.map(|s| s.as_str()).unwrap_or(fun.as_str());
                    if let Some(f) = index.get(callee).or_else(|| index.get(fun)) {
                        let ptys = materialize_mono_param_tys(&key, &f.param_tys, funs);
                        return call_site_mono_ret(f, &inferred, &ptys, index);
                    }
                    return inferred;
                }
            } else if let Some(((_, mk), _)) = renames.iter().find(|(_, n)| *n == fun) {
                let inferred = mk.ret_ty(funs, Some(fun.as_str()));
                if let Some(f) = index.get(fun) {
                    let ptys = materialize_mono_param_tys(mk, &f.param_tys, funs);
                    return call_site_mono_ret(f, &inferred, &ptys, index);
                }
                return inferred;
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
            Op::Let { value, .. } => {
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
