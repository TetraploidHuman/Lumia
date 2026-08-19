//! List-fold / float-list param upgrade after mono.

use super::local_lookup::{funref_name_of_local, infer_local_fun_ty, local_def};
use crate::ir::{Block, CoreModule, Value};
use lumia_hir::Builtin;
use lumia_hir::Sym;
use lumia_ty::Type;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// `{ y -> xs.fold(y, { a, x -> a + x }) }` with captured `List[Float]`: lift left
/// init/callback as Int; upgrade in place so codegen uses `fadd` (no mono clone
/// for env+icall arity mismatch).
pub(super) fn upgrade_captured_list_fold_float(module: &mut CoreModule) {
    // `icall` of capturing wrappers never mono-specializes list params — refine
    // `List(Int)` params from float-list call-site args before cap collection.
    upgrade_list_params_from_float_call_sites(module);

    let tables = crate::ModuleTables::from_module(module);
    let fun_ret_tys = &tables.fun_ret_tys;
    let fun_param_tys = &tables.fun_param_tys;
    let fun_cap_tys =
        super::super::float_abi::collect_fun_cap_tys(module, fun_ret_tys, fun_param_tys);
    let empty = HashMap::default();
    let by_local = &module.channel_elem_by_local;
    let module_hint = module.channel_elem_hint.as_ref();

    let mut float_cbs: HashSet<Sym> = HashSet::default();
    let mut float_outers: HashSet<Sym> = HashSet::default();
    for fun in &module.functions {
        if !fun.is_lifted_lambda() {
            continue;
        }
        let caps = fun_cap_tys.get(fun.name.as_str()).unwrap_or(&empty);
        let fold_acc_ret = block_result_is_scalar_fold_acc(&fun.body);
        collect_list_fold_float_upgrade(
            &fun.body,
            caps,
            fun_ret_tys,
            fun_param_tys,
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
            // Never coerce List → Float: list builders (`var acc = listOf(1.0); …`)
            // also end in `Name(acc)` + `Elems`, and used to be mis-classified as
            // scalar folds (printing IEEE bits across spawn/join).
            if matches!(fun.ret_ty, Type::Int | Type::Var(_)) {
                fun.ret_ty = Type::Float;
            }
        }
    }
}

/// When `f(listOf(1.0))` is an `icall` (capturing wrapper), `f`'s list param may
/// stay `List(Int)`. Lift those params so nested `AllocClosure` caps see `List[Float]`.
pub(super) fn upgrade_list_params_from_float_call_sites(module: &mut CoreModule) {
    let tables = crate::ModuleTables::from_module(module);
    let fun_ret_tys = &tables.fun_ret_tys;
    let fun_param_tys = &tables.fun_param_tys;
    let fun_cap_tys =
        super::super::float_abi::collect_fun_cap_tys(module, fun_ret_tys, fun_param_tys);
    let lifted = super::super::lifted_lambda_names(module);
    let empty = HashMap::default();

    let mut need: HashMap<Sym, HashSet<usize>> = HashMap::default();
    for fun in &module.functions {
        let caps = fun_cap_tys.get(fun.name.as_str()).unwrap_or(&empty);
        collect_float_list_call_args(
            &fun.body,
            caps,
            fun_ret_tys,
            fun_param_tys,
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

pub(super) fn collect_float_list_call_args(
    block: &Block,
    caps: &HashMap<u32, Type>,
    fun_ret_tys: &HashMap<Sym, Type>,
    fun_param_tys: &HashMap<Sym, Vec<Type>>,
    lifted: &HashSet<Sym>,
    need: &mut HashMap<Sym, HashSet<usize>>,
) {
    // Order-independent map inserts — DFS via for_each_let_value.
    crate::for_each_let_value(block, &mut |b, value| match value {
        Value::Call { fun, args } => {
            note_float_list_args(
                b,
                &fun.name,
                args,
                caps,
                fun_ret_tys,
                fun_param_tys,
                lifted,
                need,
            );
        }
        Value::IndirectCall { callee, args } => {
            if let Some(fun) = funref_name_of_local(b, callee.0) {
                note_float_list_args(
                    b,
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
    });
}

pub(super) fn note_float_list_args(
    block: &Block,
    fun: &Sym,
    args: &[crate::Local],
    caps: &HashMap<u32, Type>,
    fun_ret_tys: &HashMap<Sym, Type>,
    fun_param_tys: &HashMap<Sym, Vec<Type>>,
    lifted: &HashSet<Sym>,
    need: &mut HashMap<Sym, HashSet<usize>>,
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
            need.entry(fun.clone()).or_default().insert(i + offset);
        }
    }
}

pub(super) fn arg_is_float_list(
    block: &Block,
    id: u32,
    caps: &HashMap<u32, Type>,
    fun_ret_tys: &HashMap<Sym, Type>,
    fun_param_tys: &HashMap<Sym, Vec<Type>>,
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

pub(super) fn collect_list_fold_float_upgrade(
    block: &Block,
    caps: &HashMap<u32, Type>,
    fun_ret_tys: &HashMap<Sym, Type>,
    fun_param_tys: &HashMap<Sym, Vec<Type>>,
    channel_by_local: &HashMap<u32, Type>,
    channel_module_hint: Option<&Type>,
    outer_name: &Sym,
    fold_acc_ret: bool,
    float_cbs: &mut HashSet<Sym>,
    float_outers: &mut HashSet<Sym>,
) {
    // Order-independent set inserts — DFS via for_each_let_value.
    crate::for_each_let_value(block, &mut |b, value| {
        match value {
            Value::Builtin {
                name: Builtin::ListParFold,
                args,
                ..
            } if args.len() >= 3
                && (fold_list_arg_is_float_list(
                    b,
                    args[0].0,
                    caps,
                    fun_ret_tys,
                    fun_param_tys,
                    channel_by_local,
                    channel_module_hint,
                ) || (matches!(local_def(b, args[0].0), Some(Value::Name(_)))
                    && block_has_elems_of_float_list(
                        b,
                        caps,
                        fun_ret_tys,
                        fun_param_tys,
                        channel_by_local,
                        channel_module_hint,
                    ))) =>
            {
                float_outers.insert(outer_name.clone());
                if let Some(cb) = funref_name_of_local(b, args[2].0) {
                    float_cbs.insert(cb);
                }
            }
            // Sequential / fused `filter….fold`: `Elems(list)` + mutable acc.
            // Skip when the outer already returns a List (list builders share
            // `Name(acc)` + `Elems` with true scalar folds).
            Value::Builtin {
                name: Builtin::Elems,
                args,
                ..
            } if !args.is_empty()
                && fold_acc_ret
                && !matches!(fun_ret_tys.get(outer_name), Some(Type::List(_)))
                && fold_list_arg_is_float_list(
                    b,
                    args[0].0,
                    caps,
                    fun_ret_tys,
                    fun_param_tys,
                    channel_by_local,
                    channel_module_hint,
                ) =>
            {
                float_outers.insert(outer_name.clone());
            }
            _ => {}
        }
    });
}

pub(super) fn block_result_is_scalar_fold_acc(block: &Block) -> bool {
    let Some(r) = block.result else {
        return false;
    };
    match local_def(block, r.0) {
        Some(Value::Name(n)) => is_scalar_fold_acc_slot(n),
        _ => false,
    }
}

/// Sequential / fused fold slots (`a`, `__fuse_acc_*`). Exclude list builders
/// so map/filter/… → `List[…]` rets stay lists (not upgraded to scalar Float).
///
/// Prefixes must match HIR desugar ([`lumia_hir::LIST_BUILDER_ACC_PREFIXES`]).
pub(super) fn is_scalar_fold_acc_slot(name: &str) -> bool {
    lumia_hir::is_scalar_fold_acc_slot(name)
}

/// `flatMap` builds a mut list acc then `ListParFold(acc, …)` — the acc is a
/// `Name` load, but elems come from `Elems(captured List[Float])`.
pub(super) fn block_has_elems_of_float_list(
    block: &Block,
    caps: &HashMap<u32, Type>,
    fun_ret_tys: &HashMap<Sym, Type>,
    fun_param_tys: &HashMap<Sym, Vec<Type>>,
    channel_by_local: &HashMap<u32, Type>,
    channel_module_hint: Option<&Type>,
) -> bool {
    let mut found = false;
    crate::for_each_let_value(block, &mut |b, value| {
        if found {
            return;
        }
        if let Value::Builtin {
            name: Builtin::Elems,
            args,
            ..
        } = value
        {
            if !args.is_empty()
                && fold_list_arg_is_float_list(
                    b,
                    args[0].0,
                    caps,
                    fun_ret_tys,
                    fun_param_tys,
                    channel_by_local,
                    channel_module_hint,
                )
            {
                found = true;
            }
        }
    });
    found
}

pub(super) fn fold_list_arg_is_float_list(
    block: &Block,
    id: u32,
    caps: &HashMap<u32, Type>,
    fun_ret_tys: &HashMap<Sym, Type>,
    fun_param_tys: &HashMap<Sym, Vec<Type>>,
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
                let fl = super::super::float_abi::compute_float_locals_in_block(block);
                return !elems.is_empty() && elems.iter().all(|e| fl.contains(&e.0));
            }
            Some(Value::Call { fun, args }) => {
                if matches!(
                    fun_ret_tys.get(fun.as_str()),
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
                    | Builtin::ListSort
                    | Builtin::ListSortByKeys
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

pub(super) fn channel_recv_list_ty(
    block: &Block,
    id: u32,
    channel_by_local: &HashMap<u32, Type>,
    channel_module_hint: Option<&Type>,
) -> Option<Type> {
    super::super::float_abi::local_channel_recv_elem_ty(
        block,
        id,
        channel_by_local,
        channel_module_hint,
        None,
    )
}

pub(super) fn map_values_are_float_list(
    block: &Block,
    id: u32,
    caps: &HashMap<u32, Type>,
    fun_ret_tys: &HashMap<Sym, Type>,
    fun_param_tys: &HashMap<Sym, Vec<Type>>,
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
                let fl = super::super::float_abi::compute_float_locals_in_block(block);
                // flat: k0,v0,k1,v1,… — values at odd indices.
                return flat_pairs
                    .iter()
                    .enumerate()
                    .any(|(i, p)| i % 2 == 1 && fl.contains(&p.0));
            }
            Some(Value::Call { fun, args }) => {
                if matches!(
                    fun_ret_tys.get(fun.as_str()),
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
