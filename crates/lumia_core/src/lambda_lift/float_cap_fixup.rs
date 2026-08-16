//! After mono, patch `ClosureCap.as_float` when the captured local is Float.
//!
//! Lift runs before mono, so nested `{ x -> x + k }` inside `make(k)` still
//! sees a generic `k` and emits `closure_cap` (int). Once `make$Float` exists,
//! capture slots are Float and must load via `closure_cap_f`.

use crate::ir::{Block, CoreModule, Op, Value};
use crate::value_ty::{infer_value_ty_ctx, InferValueCtx};
use lumia_hir::Builtin;
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
    upgrade_captured_list_fold_float(module);
}

/// `{ y -> xs.fold(y, { a, x -> a + x }) }` with captured `List[Float]`: lift left
/// init/callback as Int; upgrade in place so codegen uses `fadd` (no mono clone
/// for env+icall arity mismatch).
fn upgrade_captured_list_fold_float(module: &mut CoreModule) {
    // `icall` of capturing wrappers never mono-specializes list params — refine
    // `List(Int)` params from float-list call-site args before cap collection.
    upgrade_list_params_from_float_call_sites(module);

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
    let fun_cap_tys =
        super::float_abi::collect_fun_cap_tys(module, &fun_ret_tys, &fun_param_tys);
    let empty = HashMap::default();
    let by_local = &module.channel_elem_by_local;
    let module_hint = module.channel_elem_hint.as_ref();

    let mut float_cbs: HashSet<String> = HashSet::default();
    let mut float_outers: HashSet<String> = HashSet::default();
    for fun in &module.functions {
        if !fun.is_lifted_lambda() {
            continue;
        }
        let caps = fun_cap_tys.get(&fun.name).unwrap_or(&empty);
        let fold_acc_ret = block_result_is_scalar_fold_acc(&fun.body);
        collect_list_fold_float_upgrade(
            &fun.body,
            caps,
            &fun_ret_tys,
            &fun_param_tys,
            by_local,
            module_hint,
            &fun.name,
            fold_acc_ret,
            &mut float_cbs,
            &mut float_outers,
        );
    }

    for fun in &mut module.functions {
        if float_cbs.contains(&fun.name) {
            for ty in &mut fun.param_tys {
                if matches!(ty, Type::Int | Type::Var(_)) {
                    *ty = Type::Float;
                }
            }
            fun.ret_ty = Type::Float;
        }
        if float_outers.contains(&fun.name) {
            // Env (params[0]) stays; user params (fold init) → Float.
            for i in 1..fun.param_tys.len() {
                if matches!(fun.param_tys[i], Type::Int | Type::Var(_)) {
                    fun.param_tys[i] = Type::Float;
                }
            }
            if matches!(fun.ret_ty, Type::Int | Type::Var(_) | Type::List(_)) {
                fun.ret_ty = Type::Float;
            }
        }
    }
}

/// When `f(listOf(1.0))` is an `icall` (capturing wrapper), `f`'s list param may
/// stay `List(Int)`. Lift those params so nested `AllocClosure` caps see `List[Float]`.
fn upgrade_list_params_from_float_call_sites(module: &mut CoreModule) {
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
    let fun_cap_tys =
        super::float_abi::collect_fun_cap_tys(module, &fun_ret_tys, &fun_param_tys);
    let lifted = super::lifted_lambda_names(module);
    let empty = HashMap::default();

    let mut need: HashMap<String, HashSet<usize>> = HashMap::default();
    for fun in &module.functions {
        let caps = fun_cap_tys.get(&fun.name).unwrap_or(&empty);
        collect_float_list_call_args(
            &fun.body,
            caps,
            &fun_ret_tys,
            &fun_param_tys,
            &lifted,
            &mut need,
        );
    }

    for fun in &mut module.functions {
        let Some(idxs) = need.get(&fun.name) else {
            continue;
        };
        for &i in idxs {
            match fun.param_tys.get_mut(i) {
                Some(Type::List(e)) if matches!(e.as_ref(), Type::Int | Type::Var(_)) => {
                    *e = Box::new(Type::Float);
                }
                // Capturing wrappers often type list params as bare `Int` (heap ptr).
                Some(ty @ (Type::Int | Type::Var(_))) => {
                    *ty = Type::List(Box::new(Type::Float));
                }
                _ => {}
            }
        }
    }
}

fn collect_float_list_call_args(
    block: &Block,
    caps: &HashMap<u32, Type>,
    fun_ret_tys: &HashMap<String, Type>,
    fun_param_tys: &HashMap<String, Vec<Type>>,
    lifted: &HashSet<String>,
    need: &mut HashMap<String, HashSet<usize>>,
) {
    for op in &block.ops {
        match op {
            Op::Let { value, .. } => {
                match value {
                    Value::Call { fun, args } => {
                        note_float_list_args(
                            block,
                            fun,
                            args,
                            caps,
                            fun_ret_tys,
                            fun_param_tys,
                            lifted,
                            need,
                        );
                    }
                    Value::IndirectCall { callee, args } => {
                        if let Some(fun) = funref_name_of_local(block, callee.0) {
                            note_float_list_args(
                                block,
                                &fun,
                                args,
                                caps,
                                fun_ret_tys,
                                fun_param_tys,
                                lifted,
                                need,
                            );
                        }
                    }
                    _ => {}
                }
                crate::for_each_nested_block(value, &mut |b| {
                    collect_float_list_call_args(b, caps, fun_ret_tys, fun_param_tys, lifted, need);
                });
            }
            _ => {}
        }
    }
}

fn note_float_list_args(
    block: &Block,
    fun: &str,
    args: &[crate::Local],
    caps: &HashMap<u32, Type>,
    fun_ret_tys: &HashMap<String, Type>,
    fun_param_tys: &HashMap<String, Vec<Type>>,
    lifted: &HashSet<String>,
    need: &mut HashMap<String, HashSet<usize>>,
) {
    let params = fun_param_tys.get(fun).map(|p| p.as_slice()).unwrap_or(&[]);
    // Closure env is params[0]; user args align to params[1..] when present.
    let offset = if lifted.contains(fun) && params.len() == args.len() + 1 {
        1
    } else {
        0
    };
    for (i, a) in args.iter().enumerate() {
        if arg_is_float_list(block, a.0, caps, fun_ret_tys, fun_param_tys) {
            need.entry(fun.to_string())
                .or_default()
                .insert(i + offset);
        }
    }
}

fn arg_is_float_list(
    block: &Block,
    id: u32,
    caps: &HashMap<u32, Type>,
    fun_ret_tys: &HashMap<String, Type>,
    fun_param_tys: &HashMap<String, Vec<Type>>,
) -> bool {
    fold_list_arg_is_float_list(
        block,
        id,
        caps,
        fun_ret_tys,
        fun_param_tys,
        &HashMap::default(),
        None,
    )
}

fn collect_list_fold_float_upgrade(
    block: &Block,
    caps: &HashMap<u32, Type>,
    fun_ret_tys: &HashMap<String, Type>,
    fun_param_tys: &HashMap<String, Vec<Type>>,
    channel_by_local: &HashMap<u32, Type>,
    channel_module_hint: Option<&Type>,
    outer_name: &str,
    fold_acc_ret: bool,
    float_cbs: &mut HashSet<String>,
    float_outers: &mut HashSet<String>,
) {
    for op in &block.ops {
        match op {
            Op::Let { value, .. } => {
                match value {
                    Value::Builtin {
                        name: Builtin::ListParFold,
                        args,
                        ..
                    } if args.len() >= 3
                        && (fold_list_arg_is_float_list(
                            block,
                            args[0].0,
                            caps,
                            fun_ret_tys,
                            fun_param_tys,
                            channel_by_local,
                            channel_module_hint,
                        ) || (matches!(local_def(block, args[0].0), Some(Value::Name(_)))
                            && block_has_elems_of_float_list(
                                block,
                                caps,
                                fun_ret_tys,
                                fun_param_tys,
                                channel_by_local,
                                channel_module_hint,
                            ))) =>
                    {
                        float_outers.insert(outer_name.to_string());
                        if let Some(cb) = funref_name_of_local(block, args[2].0) {
                            float_cbs.insert(cb);
                        }
                    }
                    // Sequential / fused `filter….fold`: `Elems(list)` + mutable acc.
                    Value::Builtin {
                        name: Builtin::Elems,
                        args,
                        ..
                    } if !args.is_empty()
                        && fold_acc_ret
                        && fold_list_arg_is_float_list(
                            block,
                            args[0].0,
                            caps,
                            fun_ret_tys,
                            fun_param_tys,
                            channel_by_local,
                            channel_module_hint,
                        ) =>
                    {
                        float_outers.insert(outer_name.to_string());
                    }
                    _ => {}
                }
                crate::for_each_nested_block(value, &mut |b| {
                    collect_list_fold_float_upgrade(
                        b,
                        caps,
                        fun_ret_tys,
                        fun_param_tys,
                        channel_by_local,
                        channel_module_hint,
                        outer_name,
                        fold_acc_ret,
                        float_cbs,
                        float_outers,
                    );
                });
            }
            _ => {}
        }
    }
}

fn block_result_is_scalar_fold_acc(block: &Block) -> bool {
    let Some(r) = block.result else {
        return false;
    };
    match local_def(block, r.0) {
        Some(Value::Name(n)) => is_scalar_fold_acc_slot(n),
        _ => false,
    }
}

/// Sequential / fused fold slots (`a`, `__fuse_acc_*`). Exclude list builders
/// (`__map_acc`, `__fmap_acc`, `__tolist_acc`, …) so map→List[Fun] rets stay lists.
fn is_scalar_fold_acc_slot(name: &str) -> bool {
    if name.starts_with("__fuse_acc") {
        return true;
    }
    !(name.starts_with("__map_acc")
        || name.starts_with("__fmap_acc")
        || name.starts_with("__tolist_acc")
        || name.starts_with("__filter_acc")
        || name.starts_with("__i_"))
}

/// `flatMap` builds a mut list acc then `ListParFold(acc, …)` — the acc is a
/// `Name` load, but elems come from `Elems(captured List[Float])`.
fn block_has_elems_of_float_list(
    block: &Block,
    caps: &HashMap<u32, Type>,
    fun_ret_tys: &HashMap<String, Type>,
    fun_param_tys: &HashMap<String, Vec<Type>>,
    channel_by_local: &HashMap<u32, Type>,
    channel_module_hint: Option<&Type>,
) -> bool {
    for op in &block.ops {
        match op {
            Op::Let { value, .. } => {
                if let Value::Builtin {
                    name: Builtin::Elems,
                    args,
                    ..
                } = value
                {
                    if !args.is_empty()
                        && fold_list_arg_is_float_list(
                            block,
                            args[0].0,
                            caps,
                            fun_ret_tys,
                            fun_param_tys,
                            channel_by_local,
                            channel_module_hint,
                        )
                    {
                        return true;
                    }
                }
                let mut found = false;
                crate::for_each_nested_block(value, &mut |b| {
                    if !found
                        && block_has_elems_of_float_list(
                            b,
                            caps,
                            fun_ret_tys,
                            fun_param_tys,
                            channel_by_local,
                            channel_module_hint,
                        )
                    {
                        found = true;
                    }
                });
                if found {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn fold_list_arg_is_float_list(
    block: &Block,
    id: u32,
    caps: &HashMap<u32, Type>,
    fun_ret_tys: &HashMap<String, Type>,
    fun_param_tys: &HashMap<String, Vec<Type>>,
    channel_by_local: &HashMap<u32, Type>,
    channel_module_hint: Option<&Type>,
) -> bool {
    let mut cur = id;
    let mut seen = HashSet::default();
    for _ in 0..24 {
        if !seen.insert(cur) {
            return false;
        }
        match local_def(block, cur) {
            Some(Value::Local(crate::Local(src))) => cur = *src,
            Some(Value::ClosureCap { index, .. }) => {
                return matches!(
                    caps.get(index),
                    Some(Type::List(e) | Type::Set(e)) if matches!(e.as_ref(), Type::Float)
                );
            }
            Some(Value::AllocList { elems, .. }) => {
                let fl = super::float_abi::compute_float_locals_in_block(block);
                return !elems.is_empty() && elems.iter().all(|e| fl.contains(&e.0));
            }
            Some(Value::Call { fun, args }) => {
                if matches!(
                    fun_ret_tys.get(fun),
                    Some(Type::List(e)) if matches!(e.as_ref(), Type::Float)
                ) {
                    return true;
                }
                // `id(xs)` / poly wrap: chase the list argument.
                if let Some(a) = args.first() {
                    cur = a.0;
                    continue;
                }
                return false;
            }
            Some(Value::IndirectCall { callee, args }) => {
                if let Some(Type::Fun(_, ret, _)) =
                    infer_local_fun_ty(block, callee.0, caps, fun_ret_tys, fun_param_tys)
                {
                    if matches!(ret.as_ref(), Type::List(e) if matches!(e.as_ref(), Type::Float)) {
                        return true;
                    }
                }
                if let Some(a) = args.first() {
                    cur = a.0;
                    continue;
                }
                return false;
            }
            Some(Value::Builtin {
                name:
                    Builtin::ListTake
                    | Builtin::ListSlice
                    | Builtin::ListReverse
                    | Builtin::ListConcat
                    | Builtin::ListAppend
                    | Builtin::Elems
                    | Builtin::MapKeys
                    | Builtin::ListParMap,
                args,
                ..
            }) if !args.is_empty() => {
                cur = args[0].0;
                continue;
            }
            Some(Value::Builtin {
                name: Builtin::MapValues,
                args,
                ..
            }) if !args.is_empty() => {
                return map_values_are_float_list(
                    block,
                    args[0].0,
                    caps,
                    fun_ret_tys,
                    fun_param_tys,
                    channel_by_local,
                    channel_module_hint,
                );
            }
            Some(Value::Builtin {
                name: Builtin::ChannelRecv,
                args,
                ..
            }) if !args.is_empty() => {
                return matches!(
                    channel_recv_list_ty(block, cur, channel_by_local, channel_module_hint),
                    Some(Type::List(e)) if matches!(e.as_ref(), Type::Float)
                );
            }
            Some(Value::Builtin {
                name: Builtin::AdtField,
                args,
                ..
            }) if !args.is_empty() => {
                // Unwrap `Some`/`Ok` only helps when the ADT itself is the list
                // carrier; field payload typing is handled via cap collection.
                // Fall through: not a list root.
                return false;
            }
            Some(Value::If {
                then_block,
                else_block,
                ..
            }) => {
                let then_ok = then_block.result.is_some_and(|r| {
                    fold_list_arg_is_float_list(
                        then_block,
                        r.0,
                        caps,
                        fun_ret_tys,
                        fun_param_tys,
                        channel_by_local,
                        channel_module_hint,
                    ) || fold_list_arg_is_float_list(
                        block,
                        r.0,
                        caps,
                        fun_ret_tys,
                        fun_param_tys,
                        channel_by_local,
                        channel_module_hint,
                    )
                });
                let else_ok = else_block.result.is_some_and(|r| {
                    fold_list_arg_is_float_list(
                        else_block,
                        r.0,
                        caps,
                        fun_ret_tys,
                        fun_param_tys,
                        channel_by_local,
                        channel_module_hint,
                    ) || fold_list_arg_is_float_list(
                        block,
                        r.0,
                        caps,
                        fun_ret_tys,
                        fun_param_tys,
                        channel_by_local,
                        channel_module_hint,
                    )
                });
                return then_ok || else_ok;
            }
            Some(Value::Name(_)) => {
                return false;
            }
            _ => return false,
        }
    }
    false
}

fn channel_recv_list_ty(
    block: &Block,
    id: u32,
    channel_by_local: &HashMap<u32, Type>,
    channel_module_hint: Option<&Type>,
) -> Option<Type> {
    super::float_abi::local_channel_recv_elem_ty(
        block,
        id,
        channel_by_local,
        channel_module_hint,
        None,
    )
}

fn map_values_are_float_list(
    block: &Block,
    id: u32,
    caps: &HashMap<u32, Type>,
    fun_ret_tys: &HashMap<String, Type>,
    fun_param_tys: &HashMap<String, Vec<Type>>,
    channel_by_local: &HashMap<u32, Type>,
    channel_module_hint: Option<&Type>,
) -> bool {
    let mut cur = id;
    let mut seen = HashSet::default();
    for _ in 0..16 {
        if !seen.insert(cur) {
            return false;
        }
        match local_def(block, cur) {
            Some(Value::Local(crate::Local(src))) => cur = *src,
            Some(Value::ClosureCap { index, .. }) => {
                return matches!(
                    caps.get(index),
                    Some(Type::Map(_, v)) if matches!(v.as_ref(), Type::Float)
                );
            }
            Some(Value::AllocMap { flat_pairs, .. }) => {
                let fl = super::float_abi::compute_float_locals_in_block(block);
                // flat: k0,v0,k1,v1,… — values at odd indices.
                return flat_pairs
                    .iter()
                    .enumerate()
                    .any(|(i, p)| i % 2 == 1 && fl.contains(&p.0));
            }
            Some(Value::Call { fun, args }) => {
                if matches!(
                    fun_ret_tys.get(fun),
                    Some(Type::Map(_, v)) if matches!(v.as_ref(), Type::Float)
                ) {
                    return true;
                }
                if let Some(a) = args.first() {
                    cur = a.0;
                    continue;
                }
                return false;
            }
            Some(Value::IndirectCall { callee, args }) => {
                if let Some(Type::Fun(_, ret, _)) =
                    infer_local_fun_ty(block, callee.0, caps, fun_ret_tys, fun_param_tys)
                {
                    if matches!(ret.as_ref(), Type::Map(_, v) if matches!(v.as_ref(), Type::Float))
                    {
                        return true;
                    }
                }
                if let Some(a) = args.first() {
                    cur = a.0;
                    continue;
                }
                return false;
            }
            Some(Value::Builtin {
                name: Builtin::ChannelRecv,
                ..
            }) => {
                return matches!(
                    channel_recv_list_ty(block, cur, channel_by_local, channel_module_hint),
                    Some(Type::Map(_, v)) if matches!(v.as_ref(), Type::Float)
                );
            }
            _ => return false,
        }
    }
    false
}

fn infer_local_fun_ty(
    block: &Block,
    id: u32,
    caps: &HashMap<u32, Type>,
    fun_ret_tys: &HashMap<String, Type>,
    fun_param_tys: &HashMap<String, Vec<Type>>,
) -> Option<Type> {
    let mut cur = id;
    let mut seen = HashSet::default();
    for _ in 0..16 {
        if !seen.insert(cur) {
            return None;
        }
        match local_def(block, cur)? {
            Value::Local(crate::Local(src)) => cur = *src,
            Value::ClosureCap { index, .. } => return caps.get(index).cloned(),
            Value::FunRef(n) | Value::AllocClosure { fun: n, .. } => {
                return super::fun_ty_from_tables(n, fun_ret_tys, fun_param_tys, &HashSet::default());
            }
            _ => return None,
        }
    }
    None
}

fn local_def<'a>(block: &'a Block, id: u32) -> Option<&'a Value> {
    for op in &block.ops {
        match op {
            Op::Let { local, value, .. } => {
                if local.0 == id {
                    return Some(value);
                }
                if let Some(v) = local_def_in_value(value, id) {
                    return Some(v);
                }
            }
            _ => {}
        }
    }
    None
}

fn local_def_in_value<'a>(value: &'a Value, id: u32) -> Option<&'a Value> {
    match value {
        Value::If {
            then_block,
            else_block,
            ..
        } => local_def(then_block, id).or_else(|| local_def(else_block, id)),
        Value::Loop {
            header,
            body,
            latch,
        } => local_def(header, id)
            .or_else(|| local_def(body, id))
            .or_else(|| local_def(latch, id)),
        Value::Lambda { body, .. } => local_def(body, id),
        _ => None,
    }
}

fn funref_name_of_local(block: &Block, id: u32) -> Option<String> {
    let mut cur = id;
    let mut seen = HashSet::default();
    for _ in 0..16 {
        if !seen.insert(cur) {
            return None;
        }
        match local_def(block, cur)? {
            Value::Local(crate::Local(src)) => cur = *src,
            Value::FunRef(n) | Value::AllocClosure { fun: n, .. } => return Some(n.clone()),
            _ => return None,
        }
    }
    None
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
            if !fun.is_lifted_lambda() {
                continue;
            }
            // Float/Bool/Unit are final. String/Char may still upgrade to Float
            // (`Err("e") alt 9.5`: then=AdtField(String), else=Float).
            // Fun rets still refine (e.g. Fun([Int],List(Int)) → Fun([Float],Float)).
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
                        Type::Float
                        | Type::Bool
                        | Type::Unit
                        | Type::Fun(_, _, _)
                        | Type::String
                        | Type::Char => {
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
                            Type::Float
                            | Type::Fun(_, _, _)
                            | Type::String
                            | Type::Char => new_ty = Some(t),
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
            _ => {}
        }
    }
}

fn collect_lam_caps(block: &Block, lam_caps: &mut HashMap<String, Vec<crate::Local>>) {
    for op in &block.ops {
        match op {
            Op::Let { value, .. } => {
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
        .filter(|f| f.is_lifted_lambda())
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
                    Type::Float
                        | Type::Bool
                        | Type::Fun(_, _, _)
                        | Type::Unit
                        | Type::String
                        | Type::Char
                ) || params
                    .iter()
                    .any(|t| {
                        matches!(
                            t,
                            Type::Float | Type::Fun(_, _, _) | Type::Bool | Type::String | Type::Char
                        )
                    })
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
            Op::Let { value, .. } => {
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
