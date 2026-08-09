//! Lambda lifting and capture analysis.

use crate::ir::{
    max_local_in_module, rewrite_block_locals, Block, CoreFun, CoreModule, Local, Op, Value,
};
use crate::visit::{collect_uses, for_each_nested_block};
use lumia_hir::Builtin;
use lumia_ty::{Effect, Type};
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

fn params_used_as_float(block: &Block, params: &[Local]) -> HashSet<u32> {
    let param_set: HashSet<u32> = params.iter().map(|p| p.0).collect();
    let mut float_locals: HashSet<u32> = HashSet::default();
    let mut used: HashSet<u32> = HashSet::default();
    mark_float_uses(block, &param_set, &mut float_locals, &mut used);
    used
}

fn mark_float_uses(
    block: &Block,
    params: &HashSet<u32>,
    float_locals: &mut HashSet<u32>,
    used: &mut HashSet<u32>,
) {
    for op in &block.ops {
        match op {
            Op::Let { local, value, .. } => {
                mark_float_in_value(value, params, float_locals, used);
                if value_is_float_producing(value, float_locals) {
                    float_locals.insert(local.0);
                }
            }
            Op::Effect { value } => mark_float_in_value(value, params, float_locals, used),
            _ => {}
        }
    }
}

fn mark_float_in_value(
    v: &Value,
    params: &HashSet<u32>,
    float_locals: &mut HashSet<u32>,
    used: &mut HashSet<u32>,
) {
    match v {
        Value::Binary { left, right, .. } => {
            let lf = float_locals.contains(&left.0);
            let rf = float_locals.contains(&right.0);
            if lf || rf {
                touch_param(left.0, params, used);
                touch_param(right.0, params, used);
            }
        }
        Value::Unary { operand, .. } => {
            if float_locals.contains(&operand.0) {
                touch_param(operand.0, params, used);
            }
        }
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            mark_float_uses(then_block, params, float_locals, used);
            mark_float_uses(else_block, params, float_locals, used);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            mark_float_uses(header, params, float_locals, used);
            mark_float_uses(body, params, float_locals, used);
            mark_float_uses(latch, params, float_locals, used);
        }
        _ => {}
    }
}

fn touch_param(id: u32, params: &HashSet<u32>, used: &mut HashSet<u32>) {
    if params.contains(&id) {
        used.insert(id);
    }
}

fn value_is_float_producing(v: &Value, float_locals: &HashSet<u32>) -> bool {
    match v {
        Value::Float(_) => true,
        Value::Local(Local(id)) => float_locals.contains(id),
        Value::ClosureCap { as_float: true, .. } => true,
        Value::Binary { left, right, .. } => {
            float_locals.contains(&left.0) || float_locals.contains(&right.0)
        }
        Value::Unary { operand, .. } => float_locals.contains(&operand.0),
        _ => false,
    }
}

fn block_result_is_float(block: &Block) -> bool {
    let Some(Local(r)) = block.result else {
        return false;
    };
    let mut float_locals: HashSet<u32> = HashSet::default();
    for op in &block.ops {
        if let Op::Let { local, value, .. } = op {
            if value_is_float_producing(value, &float_locals) || matches!(value, Value::Float(_)) {
                float_locals.insert(local.0);
            }
            if matches!(value, Value::Float(_)) {
                float_locals.insert(local.0);
            }
            // Propagate through float binaries more carefully:
            if let Value::Binary { left, right, .. } = value {
                if float_locals.contains(&left.0) || float_locals.contains(&right.0) {
                    float_locals.insert(local.0);
                }
            }
            if let Value::Local(Local(src)) = value {
                if float_locals.contains(src) {
                    float_locals.insert(local.0);
                }
            }
        }
    }
    float_locals.contains(&r)
}

/// Locals that hold Float values in `block` (for closure-capture ABI).
fn compute_float_locals_in_block(block: &Block) -> HashSet<u32> {
    let mut float_locals: HashSet<u32> = HashSet::default();
    for op in &block.ops {
        if let Op::Let { local, value, .. } = op {
            if value_is_float_producing(value, &float_locals) || matches!(value, Value::Float(_)) {
                float_locals.insert(local.0);
            }
            if let Value::Binary { left, right, .. } = value {
                if float_locals.contains(&left.0) || float_locals.contains(&right.0) {
                    float_locals.insert(local.0);
                }
            }
            if let Value::Local(Local(src)) = value {
                if float_locals.contains(src) {
                    float_locals.insert(local.0);
                }
            }
            if let Value::ClosureCap { as_float: true, .. } = value {
                float_locals.insert(local.0);
            }
            if let Value::Unary { operand, .. } = value {
                if float_locals.contains(&operand.0) {
                    float_locals.insert(local.0);
                }
            }
            if let Value::If {
                then_block,
                else_block,
                ..
            } = value
            {
                float_locals.extend(compute_float_locals_in_block(then_block));
                float_locals.extend(compute_float_locals_in_block(else_block));
            }
        }
    }
    float_locals
}

/// Whether the block result may be a heap pointer. `extra_params` covers lambda
/// formals that live on `Value::Lambda.params` rather than `body.params`.
fn block_result_may_heap_with_params(block: &Block, extra_params: &[Local]) -> bool {
    let Some(Local(r)) = block.result else {
        return false;
    };
    let mut params: HashSet<u32> = block.params.iter().map(|p| p.0).collect();
    params.extend(extra_params.iter().map(|p| p.0));
    local_may_heap(block, r, &params, &mut HashSet::default())
}

/// Follow `let x = y` aliases. Params are treated as maybe-heap so identity
/// lambdas like `{ s -> s }` keep a heap `ret_ty` for GC rooting at call sites.
fn local_may_heap(block: &Block, id: u32, params: &HashSet<u32>, seen: &mut HashSet<u32>) -> bool {
    if !seen.insert(id) {
        return true;
    }
    if params.contains(&id) {
        return true;
    }
    for op in &block.ops {
        if let Op::Let { local, value, .. } = op {
            if local.0 == id {
                return value_may_heap(block, value, params, seen);
            }
        }
    }
    false
}

fn value_may_heap(
    block: &Block,
    v: &Value,
    params: &HashSet<u32>,
    seen: &mut HashSet<u32>,
) -> bool {
    match v {
        Value::Local(Local(id)) => local_may_heap(block, *id, params, seen),
        Value::String(_)
        | Value::Char(_)
        | Value::AllocList { .. }
        | Value::AllocSet { .. }
        | Value::AllocMap { .. }
        | Value::AllocAdt { .. }
        | Value::AllocClosure { .. }
        | Value::ClosureCap { .. }
        | Value::FunRef(_) => true,
        Value::Builtin { name, .. } => !matches!(
            name,
            Builtin::ListLen
                | Builtin::Contains
                | Builtin::Println
                | Builtin::PrintlnInt
                | Builtin::PrintlnStr
                | Builtin::Assert
        ),
        Value::Call { .. } | Value::IndirectCall { .. } => true,
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            // Nested blocks inherit lambda/outer params for alias tracking.
            result_may_heap_inherited(then_block, params)
                || result_may_heap_inherited(else_block, params)
        }
        _ => false,
    }
}

fn result_may_heap_inherited(block: &Block, inherited: &HashSet<u32>) -> bool {
    let Some(Local(r)) = block.result else {
        return false;
    };
    let mut params = inherited.clone();
    params.extend(block.params.iter().map(|p| p.0));
    local_may_heap(block, r, &params, &mut HashSet::default())
}

/// Lift nested `Value::Lambda` to top-level `__lam_N` functions.
/// Captures (free locals / outer `var` loads) become a heap closure env.
pub(crate) fn lift_lambdas(module: &mut CoreModule) {
    let mut extras = Vec::new();
    let mut id = 0u32;
    let mut next_local = max_local_in_module(module).saturating_add(1);
    for fun in &mut module.functions {
        let mut float_locals = compute_float_locals_in_block(&fun.body);
        for (i, ty) in fun.param_tys.iter().enumerate() {
            if matches!(ty, Type::Float) {
                if let Some(p) = fun.params.get(i) {
                    float_locals.insert(p.0);
                }
            }
        }
        lift_block(
            &mut fun.body,
            &mut extras,
            &mut id,
            &mut next_local,
            &mut float_locals,
        );
    }
    module.functions.append(&mut extras);
}

fn lift_block(
    block: &mut Block,
    extras: &mut Vec<CoreFun>,
    id: &mut u32,
    next_local: &mut u32,
    float_locals: &mut HashSet<u32>,
) {
    let mut new_ops = Vec::with_capacity(block.ops.len());
    for mut op in std::mem::take(&mut block.ops) {
        match &mut op {
            Op::Let {
                value, pure_region, ..
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
                );
                new_ops.append(&mut prelude);
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
                );
                new_ops.append(&mut prelude);
            }
            Op::Assign { .. } | Op::Break | Op::Continue | Op::Return { .. } => {}
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
) {
    match value {
        Value::Lambda { params, body } => {
            lift_block(body, extras, id, next_local, float_locals);
            let (free_locals, free_names) = analyze_captures(body, params);
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
                captures.push(tmp);
                name_remap.insert(n.clone(), tmp);
            }

            if captures.is_empty() {
                let param_names: Vec<String> = (0..params.len()).map(|i| format!("p{i}")).collect();
                let (param_tys, ret_ty) = lambda_param_ret_tys(params, body);
                extras.push(CoreFun {
                    name: name.clone(),
                    params: params.clone(),
                    param_names,
                    param_tys,
                    body: *body.clone(),
                    ret_ty,
                    effect: Effect::pure(),
                    is_main: false,
                    memo: None,
                    external: None,
                    escaping: HashSet::default(),
                    // Local let-poly / nested lambdas: specialize at ground call sites.
                    scheme_poly: true,
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
                let as_float = float_locals.contains(&cap_src.0);
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
                let name_hit = name_remap
                    .iter()
                    .find(|(_, l)| l.0 == cap_src.0)
                    .map(|(n, _)| n.clone());
                if let Some(name) = name_hit {
                    name_remap.insert(name, loaded);
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
                effect: Effect::pure(),
                is_main: false,
                memo: None,
                external: None,
                escaping: HashSet::default(),
                scheme_poly: true,
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
            lift_block(then_block, extras, id, next_local, float_locals);
            lift_block(else_block, extras, id, next_local, float_locals);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            lift_block(header, extras, id, next_local, float_locals);
            lift_block(body, extras, id, next_local, float_locals);
            lift_block(latch, extras, id, next_local, float_locals);
        }
        _ => {}
    }
}

fn analyze_captures(body: &Block, params: &[Local]) -> (Vec<Local>, Vec<String>) {
    let mut defined = HashSet::default();
    for p in params {
        defined.insert(p.0);
    }
    collect_defined_locals(body, &mut defined);
    let mut used_locals = HashSet::default();
    let mut used_names = HashSet::default();
    collect_uses(body, &mut used_locals, &mut used_names);
    let mut free_locals: Vec<Local> = used_locals
        .into_iter()
        .filter(|id| !defined.contains(id))
        .map(Local)
        .collect();
    free_locals.sort_by_key(|l| l.0);
    let mut free_names: Vec<String> = used_names.into_iter().collect();
    free_names.sort();
    (free_locals, free_names)
}

fn collect_defined_locals(block: &Block, defined: &mut HashSet<u32>) {
    for p in &block.params {
        defined.insert(p.0);
    }
    for op in &block.ops {
        match op {
            Op::Let { local, value, .. } => {
                defined.insert(local.0);
                collect_defined_in_value(value, defined);
            }
            Op::Effect { value, .. } => collect_defined_in_value(value, defined),
            Op::Assign { .. } | Op::Break | Op::Continue | Op::Return { .. } => {}
        }
    }
}

fn collect_defined_in_value(value: &Value, defined: &mut HashSet<u32>) {
    if let Value::Lambda { params, .. } = value {
        for p in params {
            defined.insert(p.0);
        }
    }
    for_each_nested_block(value, &mut |b| collect_defined_locals(b, defined));
}

fn rewrite_block_names(block: &mut Block, name_remap: &HashMap<String, Local>) {
    if name_remap.is_empty() {
        return;
    }
    for op in &mut block.ops {
        match op {
            Op::Let { value, .. } | Op::Effect { value, .. } => {
                rewrite_value_names(value, name_remap);
            }
            Op::Assign { .. } | Op::Break | Op::Continue | Op::Return { .. } => {}
        }
    }
}

fn rewrite_value_names(value: &mut Value, name_remap: &HashMap<String, Local>) {
    match value {
        Value::Name(n) => {
            if let Some(l) = name_remap.get(n) {
                *value = Value::Local(*l);
            }
        }
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            rewrite_block_names(then_block, name_remap);
            rewrite_block_names(else_block, name_remap);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            rewrite_block_names(header, name_remap);
            rewrite_block_names(body, name_remap);
            rewrite_block_names(latch, name_remap);
        }
        Value::Lambda { body, .. } => rewrite_block_names(body, name_remap),
        _ => {}
    }
}
