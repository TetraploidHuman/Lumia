//! Rewrite nested `Value::Lambda` into top-level `__lam_N` functions.

use super::captures::analyze_captures;
use super::float_abi::{
    block_result_callee_ty, block_result_fun_ty, block_result_heap_ty,
    block_result_icall_cap_ty, block_result_is_float, block_result_known_hof_ty,
    compute_float_locals_in_block, float_closure_cap_indices, HofSets,
    params_used_as_float_with_caps,
};
use super::heap::block_result_may_heap_with_params;
use crate::ir::{Block, CoreFun, CoreModule, ForeignAbi, FunKind, Local, Op, Value};
use crate::visit::{
    block_has_io, for_each_op_value_mut, max_local_in_module,
    rewrite_block_locals,
};
use lumia_ty::{Effect, Type};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// Infer per-parameter / return ABI for lifted lambdas.
/// Avoids the old bug: “body mentions any float ⇒ every param is Float”.
fn lambda_param_ret_tys(
    params: &[Local],
    body: &Block,
    fun_ret_tys: &HashMap<String, Type>,
    fun_param_tys: &HashMap<String, Vec<Type>>,
    hof: &HofSets,
    cap_srcs: &[Local],
    funref_locals: &HashMap<u32, String>,
    float_cap_idxs: &HashMap<String, HashSet<u32>>,
) -> (Vec<Type>, Type) {
    let float_params = params_used_as_float_with_caps(body, params, float_cap_idxs);
    let param_tys = params
        .iter()
        .map(|p| {
            if float_params.contains(&p.0) {
                Type::Float
            } else {
                Type::Int
            }
        })
        .collect();
    let cap_funs: HashMap<u32, String> = cap_srcs
        .iter()
        .enumerate()
        .filter_map(|(i, src)| funref_locals.get(&src.0).cloned().map(|n| (i as u32, n)))
        .collect();
    let ret_ty = if block_result_is_float(body, fun_ret_tys) {
        Type::Float
    } else if super::float_abi::block_result_is_bool(body) {
        Type::Bool
    } else if super::float_abi::block_result_is_unit(body) {
        Type::Unit
    } else if let Some(t) = block_result_heap_ty(body, fun_ret_tys, fun_param_tys) {
        t
    } else if let Some(t) = block_result_callee_ty(body, fun_ret_tys) {
        t
    } else if let Some(t) =
        block_result_known_hof_ty(body, hof, fun_ret_tys, Some(&cap_funs))
    {
        t
    } else if let Some(t) =
        block_result_icall_cap_ty(body, cap_srcs, funref_locals, fun_ret_tys)
    {
        t
    } else if let Some(t) = block_result_fun_ty(body, fun_ret_tys, fun_param_tys) {
        t
    } else if block_result_may_heap_with_params(body, params) {
        // Conservative heap marker so codegen roots the Call result (§GC).
        Type::List(Box::new(Type::Int))
    } else {
        Type::Int
    };
    (param_tys, ret_ty)
}

fn lifted_effect(body: &Block, io_funs: &HashSet<String>) -> Effect {
    if block_has_io(body, io_funs) {
        Effect::io()
    } else {
        Effect::pure()
    }
}

/// Lift nested `Value::Lambda` to top-level `__lam_N` functions.
/// Captures (free locals / outer `var` loads) become a heap closure env.
pub(crate) fn lift_lambdas(module: &mut CoreModule) {
    let lifted = super::lifted_lambda_names(module);
    super::with_lifted_lambda_names(lifted, || lift_lambdas_inner(module));
}

fn lift_lambdas_inner(module: &mut CoreModule) {
    let mut extras = Vec::new();
    let mut id = 0u32;
    let mut next_local = max_local_in_module(module).saturating_add(1);
    let mut io_funs: HashSet<String> = module
        .functions
        .iter()
        .filter(|f| f.effect.has_io())
        .map(|f| f.name.clone())
        .collect();
    let (mut fun_ret_tys, mut fun_param_tys) =
        crate::ModuleTables::from_module(module).into_maps();
    let mut hof = HofSets::from_module_funs(
        module
            .functions
            .iter()
            .map(|f| (f.name.as_str(), f.params.as_slice(), &f.body)),
    );
    let mut float_cap_idxs: HashMap<String, HashSet<u32>> = module
        .functions
        .iter()
        .map(|f| (f.name.clone(), float_closure_cap_indices(&f.body)))
        .collect();
    let mut funref_locals: HashMap<u32, String> = HashMap::default();
    for fun in &mut module.functions {
        let mut float_locals = compute_float_locals_in_block(&fun.body);
        for (i, ty) in fun.param_tys.iter().enumerate() {
            if matches!(ty, Type::Float) {
                if let Some(p) = fun.params.get(i) {
                    float_locals.insert(p.0);
                }
            }
        }
        let mut float_slots = compute_float_slots(&fun.body, &float_locals);
        lift_block(
            &mut fun.body,
            &mut extras,
            &mut id,
            &mut next_local,
            &mut float_locals,
            &mut float_slots,
            &mut io_funs,
            &mut fun_ret_tys,
            &mut fun_param_tys,
            &mut hof,
            &mut funref_locals,
            &mut float_cap_idxs,
        );
    }
    module.functions.append(&mut extras);
}

/// Mutable / immutable slots that currently hold Float (`Assign` from a float local).
fn compute_float_slots(block: &Block, float_locals: &HashSet<u32>) -> HashSet<String> {
    let mut slots = HashSet::default();
    collect_float_slots(block, float_locals, &mut slots);
    slots
}

fn collect_float_slots(block: &Block, float_locals: &HashSet<u32>, slots: &mut HashSet<String>) {
    for op in &block.ops {
        match op {
            Op::Assign { name, value } => {
                if float_locals.contains(&value.0) {
                    slots.insert(name.clone());
                } else {
                    slots.remove(name);
                }
            }
            Op::Let { value, .. } => {
                crate::for_each_nested_block(value, &mut |b| {
                    collect_float_slots(b, float_locals, slots);
                });
            }
            Op::Break | Op::Continue | Op::Return { .. } => {}
        }
    }
}

fn lift_block(
    block: &mut Block,
    extras: &mut Vec<CoreFun>,
    id: &mut u32,
    next_local: &mut u32,
    float_locals: &mut HashSet<u32>,
    float_slots: &mut HashSet<String>,
    io_funs: &mut HashSet<String>,
    fun_ret_tys: &mut HashMap<String, Type>,
    fun_param_tys: &mut HashMap<String, Vec<Type>>,
    hof: &mut HofSets,
    funref_locals: &mut HashMap<u32, String>,
    float_cap_idxs: &mut HashMap<String, HashSet<u32>>,
) {
    let mut new_ops = Vec::with_capacity(block.ops.len());
    for mut op in std::mem::take(&mut block.ops) {
        match &mut op {
            Op::Let {
                local,
                value,
                pure_region,
                ..
            } => {
                let mut prelude = Vec::new();
                lift_value(
                    value,
                    extras,
                    id,
                    next_local,
                    &mut prelude,
                    *pure_region,
                    float_locals,
                    float_slots,
                    io_funs,
                    fun_ret_tys,
                    fun_param_tys,
                    hof,
                    funref_locals,
                    float_cap_idxs,
                );
                new_ops.append(&mut prelude);
                // Keep float_locals fresh for later captures in this block.
                if matches!(value, Value::Name(n) if float_slots.contains(n))
                    || super::float_abi::value_is_float_producing(value, float_locals)
                {
                    float_locals.insert(local.0);
                }
                match value {
                    Value::FunRef(name) => {
                        funref_locals.insert(local.0, name.clone());
                    }
                    Value::AllocClosure { fun, .. } => {
                        funref_locals.insert(local.0, fun.clone());
                    }
                    Value::Local(Local(src)) => {
                        if let Some(n) = funref_locals.get(src).cloned() {
                            funref_locals.insert(local.0, n);
                        } else {
                            funref_locals.remove(&local.0);
                        }
                    }
                    _ => {
                        funref_locals.remove(&local.0);
                    }
                }
            }
            Op::Assign { name, value } => {
                if float_locals.contains(&value.0) {
                    float_slots.insert(name.clone());
                } else {
                    float_slots.remove(name);
                }
            }
            Op::Break | Op::Continue | Op::Return { .. } => {}
        }
        new_ops.push(op);
    }
    block.ops = new_ops;
}

fn lift_value(
    value: &mut Value,
    extras: &mut Vec<CoreFun>,
    id: &mut u32,
    next_local: &mut u32,
    prelude: &mut Vec<Op>,
    pure_region: bool,
    float_locals: &mut HashSet<u32>,
    float_slots: &mut HashSet<String>,
    io_funs: &mut HashSet<String>,
    fun_ret_tys: &mut HashMap<String, Type>,
    fun_param_tys: &mut HashMap<String, Vec<Type>>,
    hof: &mut HofSets,
    funref_locals: &mut HashMap<u32, String>,
    float_cap_idxs: &mut HashMap<String, HashSet<u32>>,
) {
    match value {
        Value::Lambda { params, body } => {
            lift_block(
                body,
                extras,
                id,
                next_local,
                float_locals,
                float_slots,
                io_funs,
                fun_ret_tys,
                fun_param_tys,
                hof,
                funref_locals,
                float_cap_idxs,
            );
            let (free_locals, free_names) = analyze_captures(body, params);
            let assigned_names = collect_assigned_names(body);
            let name = format!("__lam_{id}");
            *id += 1;

            let mut captures = Vec::new();
            let mut remap: HashMap<u32, u32> = HashMap::default();
            let mut name_remap: HashMap<String, Local> = HashMap::default();

            for fl in &free_locals {
                captures.push(*fl);
            }
            for n in &free_names {
                let tmp = Local(*next_local);
                *next_local += 1;
                prelude.push(Op::Let {
                    local: tmp,
                    value: Value::Name(n.clone()),
                    pure_region,
                });
                if float_slots.contains(n) {
                    float_locals.insert(tmp.0);
                }
                captures.push(tmp);
                name_remap.insert(n.clone(), tmp);
            }

            if captures.is_empty() {
                let param_names: Vec<String> = (0..params.len()).map(|i| format!("p{i}")).collect();
                let (param_tys, ret_ty) =
                    lambda_param_ret_tys(params, body, fun_ret_tys, fun_param_tys, hof, &[], funref_locals, float_cap_idxs);
                let effect = lifted_effect(body, io_funs);
                if effect.has_io() {
                    io_funs.insert(name.clone());
                }
                fun_ret_tys.insert(name.clone(), ret_ty.clone());
                fun_param_tys.insert(name.clone(), param_tys.clone());
                float_cap_idxs.insert(name.clone(), float_closure_cap_indices(body));
                hof.note(&name, params, body);
                super::note_lifted_lambda_name(name.clone());
                extras.push(CoreFun {
                    name: name.clone(),
                    params: params.clone(),
                    param_names,
                    param_tys,
                    body: *body.clone(),
                    ret_ty,
                    effect,
                    is_main: false,
                    memo: None,
                    external: None,
                    foreign_abi: ForeignAbi::C,
                    escaping: HashSet::default(),
                    // Local let-poly / nested lambdas: specialize at ground call sites.
                    scheme_poly: true,
                    mono_of: None,
                    kind: FunKind::LiftedLambda,
                });
                *value = Value::FunRef(name);
                return;
            }

            let env = Local(*next_local);
            *next_local += 1;
            let mut new_body = *body.clone();
            // Map each capture slot → a fresh local loaded from env at entry.
            let mut load_ops = Vec::new();
            for (i, cap_src) in captures.iter().enumerate() {
                let loaded = Local(*next_local);
                *next_local += 1;
                let name_hit = name_remap
                    .iter()
                    .find(|(_, l)| l.0 == cap_src.0)
                    .map(|(n, _)| n.clone());
                let as_float = float_locals.contains(&cap_src.0)
                    || name_hit.as_ref().is_some_and(|n| float_slots.contains(n));
                if as_float {
                    float_locals.insert(loaded.0);
                }
                load_ops.push(Op::Let {
                    local: loaded,
                    value: Value::ClosureCap {
                        env,
                        index: i as u32,
                        as_float,
                    },
                    pure_region: true,
                });
                if let Some(name) = name_hit {
                    if assigned_names.contains(&name) {
                        // Capture-by-value: seed a local slot from the env, then
                        // keep `Name`/`Assign` on that slot so `n = n+1; n` works.
                        load_ops.push(Op::Assign {
                            name: name.clone(),
                            value: loaded,
                        });
                        if as_float {
                            float_slots.insert(name.clone());
                        }
                        name_remap.remove(&name);
                    } else {
                        name_remap.insert(name, loaded);
                    }
                } else {
                    remap.insert(cap_src.0, loaded.0);
                }
            }
            rewrite_block_locals(&mut new_body, &remap);
            rewrite_block_names(&mut new_body, &name_remap);

            let mut ops = load_ops;
            ops.append(&mut new_body.ops);
            new_body.ops = ops;

            let mut fun_params = vec![env];
            fun_params.extend(params.iter().copied());
            let mut param_names = vec!["env".into()];
            param_names.extend((0..params.len()).map(|i| format!("p{i}")));

            let (user_param_tys, ret_ty) = lambda_param_ret_tys(
                params,
                &new_body,
                fun_ret_tys,
                fun_param_tys,
                hof,
                &captures,
                funref_locals,
                float_cap_idxs,
            );
            let effect = lifted_effect(&new_body, io_funs);
            if effect.has_io() {
                io_funs.insert(name.clone());
            }
            let mut full_param_tys = vec![Type::Int]; // env pointer bits
            full_param_tys.extend(user_param_tys);
            fun_ret_tys.insert(name.clone(), ret_ty.clone());
            fun_param_tys.insert(name.clone(), full_param_tys.clone());
            float_cap_idxs.insert(name.clone(), float_closure_cap_indices(&new_body));
            hof.note(&name, &fun_params, &new_body);
            super::note_lifted_lambda_name(name.clone());
            extras.push(CoreFun {
                name: name.clone(),
                params: fun_params,
                param_names,
                param_tys: full_param_tys,
                body: new_body,
                ret_ty,
                effect,
                is_main: false,
                memo: None,
                external: None,
                foreign_abi: ForeignAbi::C,
                escaping: HashSet::default(),
                scheme_poly: true,
                mono_of: None,
                kind: FunKind::LiftedLambda,
            });
            *value = Value::AllocClosure {
                fun: name,
                captures,
            };
        }
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            lift_block(
                then_block,
                extras,
                id,
                next_local,
                float_locals,
                float_slots,
                io_funs,
                fun_ret_tys,
                fun_param_tys,
                hof,
                funref_locals,
                float_cap_idxs,
            );
            lift_block(
                else_block,
                extras,
                id,
                next_local,
                float_locals,
                float_slots,
                io_funs,
                fun_ret_tys,
                fun_param_tys,
                hof,
                funref_locals,
                float_cap_idxs,
            );
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            lift_block(
                header,
                extras,
                id,
                next_local,
                float_locals,
                float_slots,
                io_funs,
                fun_ret_tys,
                fun_param_tys,
                hof,
                funref_locals,
                float_cap_idxs,
            );
            lift_block(
                body,
                extras,
                id,
                next_local,
                float_locals,
                float_slots,
                io_funs,
                fun_ret_tys,
                fun_param_tys,
                hof,
                funref_locals,
                float_cap_idxs,
            );
            lift_block(
                latch,
                extras,
                id,
                next_local,
                float_locals,
                float_slots,
                io_funs,
                fun_ret_tys,
                fun_param_tys,
                hof,
                funref_locals,
                float_cap_idxs,
            );
        }
        _ => {}
    }
}

fn rewrite_block_names(block: &mut Block, name_remap: &HashMap<String, Local>) {
    if name_remap.is_empty() {
        return;
    }
    for_each_op_value_mut(block, &mut |value| {
        if let Value::Name(n) = value {
            if let Some(l) = name_remap.get(n) {
                *value = Value::Local(*l);
            }
        }
    });
}

/// Free names that are written with `Assign` inside `block` (capture-by-value
/// still needs a local slot for `n = n + 1; n`).
fn collect_assigned_names(block: &Block) -> HashSet<String> {
    let mut out = HashSet::default();
    crate::visit::collect_assigned_names(block, &mut out);
    out
}
