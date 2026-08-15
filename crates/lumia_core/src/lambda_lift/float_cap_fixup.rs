//! After mono, patch `ClosureCap.as_float` when the captured local is Float.
//!
//! Lift runs before mono, so nested `{ x -> x + k }` inside `make(k)` still
//! sees a generic `k` and emits `closure_cap` (int). Once `make$Float` exists,
//! capture slots are Float and must load via `closure_cap_f`.

use crate::ir::{Block, CoreModule, Op, Value};
use crate::value_ty::{infer_value_ty_ctx, InferValueCtx};
use lumia_ty::Type;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

pub(crate) fn fixup_closure_float_caps(module: &mut CoreModule) {
    let fun_ret_tys: HashMap<String, Type> = module
        .functions
        .iter()
        .map(|f| (f.name.clone(), f.ret_ty.clone()))
        .collect();
    let fun_param_tys: HashMap<String, Vec<Type>> = module
        .functions
        .iter()
        .map(|f| (f.name.clone(), f.param_tys.clone()))
        .collect();

    // (lifted_fun, capture_index) → must be float
    let mut need_float: HashSet<(String, u32)> = HashSet::default();
    for fun in &module.functions {
        let mut local_tys: HashMap<u32, Type> = HashMap::default();
        for (p, ty) in fun.params.iter().zip(fun.param_tys.iter()) {
            local_tys.insert(p.0, ty.clone());
        }
        let mut slot_tys: HashMap<String, Type> = HashMap::default();
        scan_alloc_closure_caps(
            &fun.body,
            &mut local_tys,
            &mut slot_tys,
            &fun_ret_tys,
            &fun_param_tys,
            &mut need_float,
        );
    }

    if !need_float.is_empty() {
        for fun in &mut module.functions {
            let indices: Vec<u32> = need_float
                .iter()
                .filter(|(n, _)| n == &fun.name)
                .map(|(_, i)| *i)
                .collect();
            if indices.is_empty() {
                continue;
            }
            let mut changed = false;
            patch_caps_in_block(&mut fun.body, &indices, &mut changed);
            if changed {
                // Env is params[0]; refresh user param / ret Float ABI from body.
                if fun.params.len() > 1 {
                    let user: Vec<_> = fun.params[1..].to_vec();
                    let float_ps = super::float_abi::params_used_as_float(&fun.body, &user);
                    for (i, p) in user.iter().enumerate() {
                        if float_ps.contains(&p.0) {
                            fun.param_tys[i + 1] = Type::Float;
                        }
                    }
                }
                if super::float_abi::block_result_is_float(&fun.body, &fun_ret_tys) {
                    fun.ret_ty = Type::Float;
                }
            }
        }
    }

    // Always refresh Fun/spawn rets after directize/mono — even when no
    // ClosureCap patches ran (`var f = {…}; f(1.5)` has no float env caps).
    refresh_alloc_closure_fun_rets(module);
    refresh_lifted_lambda_rets(module);
}

/// Upgrade `__lam_*` return types from callee tables / float locals after mono.
fn refresh_lifted_lambda_rets(module: &mut CoreModule) {
    let mut fun_ret_tys: HashMap<String, Type> = module
        .functions
        .iter()
        .map(|f| (f.name.clone(), f.ret_ty.clone()))
        .collect();
    let fun_param_tys: HashMap<String, Vec<Type>> = module
        .functions
        .iter()
        .map(|f| (f.name.clone(), f.param_tys.clone()))
        .collect();
    let hof = super::float_abi::HofSets::from_module_funs(
        module
            .functions
            .iter()
            .map(|f| (f.name.as_str(), f.params.as_slice(), &f.body)),
    );
    // lam → (capture_index → callee fun name) from AllocClosure sites.
    let mut cap_funs: HashMap<String, HashMap<u32, String>> = HashMap::default();
    let mut lam_caps: HashMap<String, Vec<crate::Local>> = HashMap::default();
    for fun in &module.functions {
        let mut funref_locals: HashMap<u32, String> = HashMap::default();
        collect_alloc_closure_cap_funs(&fun.body, &mut funref_locals, &mut cap_funs);
        collect_lam_caps(&fun.body, &mut lam_caps);
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
        let fun_cap_tys = super::float_abi::collect_fun_cap_tys(module, &fun_ret_tys, &fun_param_tys);
        let empty_caps = HashMap::default();
        for fun in &mut module.functions {
            if !fun.name.starts_with("__lam_") {
                continue;
            }
            // Float/Bool/Unit are final. Fun rets still refine (e.g. Fun([Int],List(Int))
            // → Fun([Float],Float) after curried-compose caps resolve).
            if matches!(fun.ret_ty, Type::Float | Type::Bool | Type::Unit) {
                continue;
            }
            let this_caps = fun_cap_tys.get(&fun.name).unwrap_or(&empty_caps);
            let mut new_ty: Option<Type> = None;
            if super::float_abi::block_result_is_float(&fun.body, &fun_ret_tys) {
                new_ty = Some(Type::Float);
            } else if super::float_abi::block_result_is_bool(&fun.body) {
                new_ty = Some(Type::Bool);
            } else if super::float_abi::block_result_is_unit(&fun.body) {
                new_ty = Some(Type::Unit);
            } else {
                let caps = lam_caps.get(&fun.name).map(|c| c.as_slice());
                if let Some(t) = super::float_abi::block_result_channel_recv_ty(
                    &fun.body,
                    &by_local,
                    module_hint.as_ref(),
                    caps,
                ) {
                    new_ty = Some(t);
                } else if let Some(t) = super::float_abi::block_result_heap_ty_caps(
                    &fun.body,
                    &fun_ret_tys,
                    &fun_param_tys,
                    this_caps,
                ) {
                    match &t {
                        Type::Float | Type::Bool | Type::Unit | Type::Fun(_, _, _) => {
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
                    let caps = cap_funs.get(&fun.name);
                    let from_call =
                        super::float_abi::block_result_callee_ty(&fun.body, &fun_ret_tys);
                    let from_apply = super::float_abi::block_result_known_hof_ty(
                        &fun.body,
                        &hof,
                        &fun_ret_tys,
                        caps,
                    );
                    let from_icall = caps.and_then(|c| {
                        super::float_abi::block_result_icall_cap_ty_by_index(
                            &fun.body,
                            c,
                            &fun_ret_tys,
                        )
                    });
                    let from_fun = super::float_abi::block_result_fun_ty(
                        &fun.body,
                        &fun_ret_tys,
                        &fun_param_tys,
                    );
                    if let Some(t) = from_call.or(from_apply).or(from_icall).or(from_fun) {
                        match &t {
                            Type::Float | Type::Fun(_, _, _) => new_ty = Some(t),
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
                    super::float_abi::prefer_concrete_heap_ty(fun.ret_ty.clone(), t);
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

fn collect_alloc_closure_cap_funs(
    block: &Block,
    funref_locals: &mut HashMap<u32, String>,
    cap_funs: &mut HashMap<String, HashMap<u32, String>>,
) {
    for op in &block.ops {
        match op {
            Op::Let { local, value, .. } => {
                match value {
                    Value::FunRef(name) => {
                        funref_locals.insert(local.0, name.clone());
                    }
                    Value::AllocClosure { fun, captures } => {
                        funref_locals.insert(local.0, fun.clone());
                        let entry = cap_funs.entry(fun.clone()).or_default();
                        for (i, cap) in captures.iter().enumerate() {
                            if let Some(n) = funref_locals.get(&cap.0) {
                                entry.insert(i as u32, n.clone());
                            }
                        }
                    }
                    Value::Local(crate::ir::Local(src)) => {
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
                crate::for_each_nested_block(value, &mut |b| {
                    collect_alloc_closure_cap_funs(b, funref_locals, cap_funs);
                });
            }
            Op::Effect { value } => {
                crate::for_each_nested_block(value, &mut |b| {
                    collect_alloc_closure_cap_funs(b, funref_locals, cap_funs);
                });
            }
            _ => {}
        }
    }
}

fn collect_lam_caps(block: &Block, lam_caps: &mut HashMap<String, Vec<crate::Local>>) {
    for op in &block.ops {
        match op {
            Op::Let { value, .. } | Op::Effect { value } => {
                if let Value::AllocClosure { fun, captures } = value {
                    lam_caps.insert(fun.clone(), captures.clone());
                }
                crate::for_each_nested_block(value, &mut |b| {
                    collect_lam_caps(b, lam_caps);
                });
            }
            _ => {}
        }
    }
}

fn refresh_alloc_closure_fun_rets(module: &mut CoreModule) {
    let mut fun_ret_tys: HashMap<String, Type> = module
        .functions
        .iter()
        .map(|f| (f.name.clone(), f.ret_ty.clone()))
        .collect();
    let _ = refresh_alloc_closure_fun_rets_round(module, &mut fun_ret_tys);
}

/// Upgrade `ret = AllocClosure(lam)` to `Fun` from `lam`'s current signature.
/// Returns whether any function ret changed.
fn refresh_alloc_closure_fun_rets_round(
    module: &mut CoreModule,
    fun_ret_tys: &mut HashMap<String, Type>,
) -> bool {
    let lam_sig: HashMap<String, (Vec<Type>, Type)> = module
        .functions
        .iter()
        .filter(|f| f.name.starts_with("__lam_"))
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
                    Type::Float | Type::Bool | Type::Fun(_, _, _) | Type::Unit
                ) || params
                    .iter()
                    .any(|t| matches!(t, Type::Float | Type::Fun(_, _, _) | Type::Bool))
                    || matches!(fun.ret_ty, Type::Fun(_, _, _));
                if !interesting {
                    continue;
                }
                let candidate = Type::Fun(params.clone(), Box::new(ret.clone()), fun.effect);
                let merged =
                    super::float_abi::prefer_concrete_heap_ty(fun.ret_ty.clone(), candidate);
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

fn result_alloc_closure_fun(block: &Block) -> Option<String> {
    let r = block.result?;
    for op in &block.ops {
        if let Op::Let {
            local,
            value: Value::AllocClosure { fun, .. },
            ..
        } = op
        {
            if *local == r {
                return Some(fun.clone());
            }
        }
        if let Op::Let {
            local,
            value: Value::Local(src),
            ..
        } = op
        {
            if *local == r {
                // Follow trivial alias.
                for op2 in &block.ops {
                    if let Op::Let {
                        local: l2,
                        value: Value::AllocClosure { fun, .. },
                        ..
                    } = op2
                    {
                        if *l2 == *src {
                            return Some(fun.clone());
                        }
                    }
                }
            }
        }
    }
    None
}

fn scan_alloc_closure_caps(
    block: &Block,
    local_tys: &mut HashMap<u32, Type>,
    slot_tys: &mut HashMap<String, Type>,
    fun_ret_tys: &HashMap<String, Type>,
    fun_param_tys: &HashMap<String, Vec<Type>>,
    need_float: &mut HashSet<(String, u32)>,
) {
    for op in &block.ops {
        match op {
            Op::Let { local, value, .. } => {
                note_alloc_caps(value, local_tys, need_float);
                let ty = infer_value_ty_ctx(
                    value,
                    InferValueCtx {
                        local_tys,
                        slot_tys: Some(slot_tys),
                        fun_ret_tys: Some(fun_ret_tys),
                        fun_param_tys: Some(fun_param_tys),
                        fun_param0_identity: None,
                        funref_locals: None,
                        local_int_consts: None,
                        sum_max_arity: None,
                        channel_elem_hint: None,
                    },
                    None,
                );
                local_tys.insert(local.0, ty);
                crate::for_each_nested_block(value, &mut |b| {
                    scan_alloc_closure_caps(
                        b,
                        local_tys,
                        slot_tys,
                        fun_ret_tys,
                        fun_param_tys,
                        need_float,
                    );
                });
            }
            Op::Assign { name, value } => {
                if let Some(ty) = local_tys.get(&value.0).cloned() {
                    slot_tys.insert(name.clone(), ty);
                }
            }
            Op::Effect { value } => {
                note_alloc_caps(value, local_tys, need_float);
                crate::for_each_nested_block(value, &mut |b| {
                    scan_alloc_closure_caps(
                        b,
                        local_tys,
                        slot_tys,
                        fun_ret_tys,
                        fun_param_tys,
                        need_float,
                    );
                });
            }
            Op::Break | Op::Continue | Op::Return { .. } => {}
        }
    }
}

fn note_alloc_caps(
    value: &Value,
    local_tys: &HashMap<u32, Type>,
    need_float: &mut HashSet<(String, u32)>,
) {
    if let Value::AllocClosure { fun, captures } = value {
        for (i, cap) in captures.iter().enumerate() {
            if matches!(local_tys.get(&cap.0), Some(Type::Float)) {
                need_float.insert((fun.clone(), i as u32));
            }
        }
    }
}

fn patch_caps_in_block(block: &mut Block, indices: &[u32], changed: &mut bool) {
    for op in &mut block.ops {
        match op {
            Op::Let { value, .. } | Op::Effect { value } => {
                patch_value_caps(value, indices, changed);
            }
            _ => {}
        }
    }
}

fn patch_value_caps(value: &mut Value, indices: &[u32], changed: &mut bool) {
    if let Value::ClosureCap {
        index, as_float, ..
    } = value
    {
        if !*as_float && indices.contains(index) {
            *as_float = true;
            *changed = true;
        }
    }
    crate::for_each_nested_block_mut(value, &mut |b| patch_caps_in_block(b, indices, changed));
}
