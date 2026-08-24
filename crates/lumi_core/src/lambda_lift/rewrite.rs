//! Rewrite nested `Value::Lambda` into top-level `__lam_N` functions.

use super::captures::analyze_captures;
use super::float_abi::{
    block_result_is_float, compute_float_locals_in_block, params_used_as_float,
};
use super::heap::block_result_may_heap_with_params;
use crate::ir::{
    max_local_in_module, rewrite_block_locals, Block, CoreFun, CoreModule, Local, Op, Value,
};
use crate::visit::{block_has_io, for_each_nested_block, for_each_op_value_mut};
use lumi_ty::{Effect, Type};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// Infer per-parameter / return ABI for lifted lambdas.
/// Avoids the old bug: “body mentions any float ⇒ every param is Float”.
fn lambda_param_ret_tys(params: &[Local], body: &Block) -> (Vec<Type>, Type) {
    let float_params = params_used_as_float(body, params);
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
    let ret_ty = if block_result_is_float(body) {
        Type::Float
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
    let mut extras = Vec::new();
    let mut id = 0u32;
    let mut next_local = max_local_in_module(module).saturating_add(1);
    let mut io_funs: HashSet<String> = module
        .functions
        .iter()
        .filter(|f| f.effect.has_io())
        .map(|f| f.name.clone())
        .collect();
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
            Op::Let { value, .. } | Op::Effect { value } => {
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
                );
                new_ops.append(&mut prelude);
                // Keep float_locals fresh for later captures in this block.
                if matches!(value, Value::Name(n) if float_slots.contains(n))
                    || super::float_abi::value_is_float_producing(value, float_locals)
                {
                    float_locals.insert(local.0);
                }
            }
            Op::Effect { value, .. } => {
                let mut prelude = Vec::new();
                lift_value(
                    value,
                    extras,
                    id,
                    next_local,
                    &mut prelude,
                    true,
                    float_locals,
                    float_slots,
                    io_funs,
                );
                new_ops.append(&mut prelude);
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
                let (param_tys, ret_ty) = lambda_param_ret_tys(params, body);
                let effect = lifted_effect(body, io_funs);
                if effect.has_io() {
                    io_funs.insert(name.clone());
                }
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
                    escaping: HashSet::default(),
                    // Local let-poly / nested lambdas: specialize at ground call sites.
                    scheme_poly: true,
                    mono_of: None,
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

            let (user_param_tys, ret_ty) = lambda_param_ret_tys(params, &new_body);
            let effect = lifted_effect(&new_body, io_funs);
            if effect.has_io() {
                io_funs.insert(name.clone());
            }
            extras.push(CoreFun {
                name: name.clone(),
                params: fun_params,
                param_names,
                param_tys: {
                    let mut tys = vec![Type::Int]; // env pointer bits
                    tys.extend(user_param_tys);
                    tys
                },
                body: new_body,
                ret_ty,
                effect,
                is_main: false,
                memo: None,
                external: None,
                escaping: HashSet::default(),
                scheme_poly: true,
                mono_of: None,
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
            );
            lift_block(
                else_block,
                extras,
                id,
                next_local,
                float_locals,
                float_slots,
                io_funs,
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
            );
            lift_block(
                body,
                extras,
                id,
                next_local,
                float_locals,
                float_slots,
                io_funs,
            );
            lift_block(
                latch,
                extras,
                id,
                next_local,
                float_locals,
                float_slots,
                io_funs,
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
    collect_assigned_names_in_block(block, &mut out);
    out
}

fn collect_assigned_names_in_block(block: &Block, out: &mut HashSet<String>) {
    for op in &block.ops {
        match op {
            Op::Assign { name, .. } => {
                out.insert(name.clone());
            }
            Op::Let { value, .. } | Op::Effect { value } => {
                for_each_nested_block(value, &mut |b| collect_assigned_names_in_block(b, out));
            }
            Op::Break | Op::Continue | Op::Return { .. } => {}
        }
    }
}
