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

    if need_float.is_empty() {
        return;
    }

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
            if super::float_abi::block_result_is_float(&fun.body) {
                fun.ret_ty = Type::Float;
            }
        }
    }

    // `make` returns `AllocClosure(__lam_0)`: once `__lam_0` is Float, refresh
    // the Fun ret so `icall` uses float ABI (not println of IEEE bits).
    refresh_alloc_closure_fun_rets(module);
}

fn refresh_alloc_closure_fun_rets(module: &mut CoreModule) {
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

    for fun in &mut module.functions {
        if let Some(lam) = result_alloc_closure_fun(&fun.body) {
            if let Some((params, ret)) = lam_sig.get(&lam) {
                if matches!(ret, Type::Float) || params.iter().any(|t| matches!(t, Type::Float)) {
                    fun.ret_ty = Type::Fun(params.clone(), Box::new(ret.clone()), fun.effect);
                }
            }
        }
    }
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
