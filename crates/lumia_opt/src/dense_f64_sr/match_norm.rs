use super::shape_util::{
    body_calls_any, for_each_shape_value, is_out_list, is_out_set, mentions_local,
    out_slot_for_list_param, param_float, param_list_f64, ret_list_f64, OutSlot,
};
use lumia_core::CoreBinOp as BinOp;
use lumia_core::{
    is_list_get, is_list_set, is_nontrivial_add_or_sub, same_local, Block, Local, Value,
};
use lumia_ty::Type;
use rustc_hash::FxHashMap as HashMap;

pub(super) fn match_sum_sq_fun(
    fun: &lumia_core::CoreFun,
    defs: &HashMap<u32, Value>,
) -> Option<()> {
    if fun.params.len() != 1 || !param_list_f64(fun, 0) || !matches!(fun.ret_ty, Type::Float) {
        return None;
    }
    let xs = fun.params[0];
    if !fun_has_sum_sq_shape(&fun.body, defs, xs) {
        return None;
    }
    if body_calls_any(&fun.body, &["lumia_f64_sqrt", "sqrtF", "sqrt"]) {
        return None;
    }
    Some(())
}

/// Arithmetic mean — get + add + div, no set/mul.
pub(super) fn match_mean_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 1 || !param_list_f64(fun, 0) || !matches!(fun.ret_ty, Type::Float) {
        return None;
    }
    let xs = fun.params[0];
    if !fun_has_mean_shape(&fun.body, defs, xs) {
        return None;
    }
    Some(())
}

/// `√(∑ xᵢ²)` via scalar `lumia_f64_sqrt` / `sqrt`.
pub(super) fn match_l2_norm_fun(
    fun: &lumia_core::CoreFun,
    defs: &HashMap<u32, Value>,
) -> Option<()> {
    if fun.params.len() != 1 || !param_list_f64(fun, 0) || !matches!(fun.ret_ty, Type::Float) {
        return None;
    }
    let xs = fun.params[0];
    if !fun_has_sum_sq_shape(&fun.body, defs, xs) {
        return None;
    }
    if !body_calls_any(&fun.body, &["lumia_f64_sqrt", "sqrtF", "sqrt"]) {
        return None;
    }
    Some(())
}

/// Population std: variance loop + sqrt (has nontrivial sub).
pub(super) fn match_std_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 1 || !param_list_f64(fun, 0) || !matches!(fun.ret_ty, Type::Float) {
        return None;
    }
    let xs = fun.params[0];
    if !fun_has_std_shape(&fun.body, defs, xs) {
        return None;
    }
    Some(())
}

/// In-place L2 normalize with `eps` (set + sqrt + mentions eps).
pub(super) fn match_l2_normalize_fun(
    fun: &lumia_core::CoreFun,
    defs: &HashMap<u32, Value>,
) -> Option<()> {
    if fun.params.len() != 2
        || !param_list_f64(fun, 0)
        || !param_float(fun, 1)
        || !ret_list_f64(fun)
    {
        return None;
    }
    let (xs, eps) = (fun.params[0], fun.params[1]);
    let body = &fun.body;
    let out = out_slot_for_list_param(body, xs)?;
    if !fun_has_l2_normalize_shape(body, defs, &out, xs, eps) {
        return None;
    }
    Some(())
}

/// Softmax: max pass + exp + normalize (set + exp call + Gt).
pub(super) fn match_softmax_fun(
    fun: &lumia_core::CoreFun,
    defs: &HashMap<u32, Value>,
) -> Option<()> {
    if fun.params.len() != 1 || !param_list_f64(fun, 0) || !ret_list_f64(fun) {
        return None;
    }
    let xs = fun.params[0];
    let body = &fun.body;
    let out = out_slot_for_list_param(body, xs)?;
    if !fun_has_softmax_shape(body, defs, &out, xs) {
        return None;
    }
    Some(())
}

fn fun_has_sum_sq_shape(body: &Block, defs: &HashMap<u32, Value>, xs: Local) -> bool {
    let mut get = false;
    let mut mul = false;
    let mut add = false;
    let mut set = false;
    let mut div = false;
    for_each_shape_value(body, defs, &mut |v| {
        if let Some((lst, _)) = is_list_get(v) {
            if same_local(lst, xs, defs) {
                get = true;
            }
        }
        if matches!(v, Value::Binary { op: BinOp::Mul, .. }) {
            mul = true;
        }
        if is_nontrivial_add_or_sub(v, defs) && matches!(v, Value::Binary { op: BinOp::Add, .. }) {
            add = true;
        }
        if matches!(v, Value::Binary { op: BinOp::Div, .. }) {
            div = true;
        }
        if is_list_set(v).is_some() {
            set = true;
        }
    });
    get && mul && add && !set && !div
}

fn fun_has_mean_shape(body: &Block, defs: &HashMap<u32, Value>, xs: Local) -> bool {
    let mut get = false;
    let mut add = false;
    let mut div = false;
    let mut mul = false;
    let mut set = false;
    for_each_shape_value(body, defs, &mut |v| {
        if let Some((lst, _)) = is_list_get(v) {
            if same_local(lst, xs, defs) {
                get = true;
            }
        }
        if is_nontrivial_add_or_sub(v, defs) && matches!(v, Value::Binary { op: BinOp::Add, .. }) {
            add = true;
        }
        if matches!(v, Value::Binary { op: BinOp::Div, .. }) {
            div = true;
        }
        if matches!(v, Value::Binary { op: BinOp::Mul, .. }) {
            mul = true;
        }
        if is_list_set(v).is_some() {
            set = true;
        }
    });
    get && add && div && !mul && !set
}

fn fun_has_std_shape(body: &Block, defs: &HashMap<u32, Value>, xs: Local) -> bool {
    let mut get = false;
    let mut sub = false;
    let mut mul = false;
    let mut div = false;
    let mut set = false;
    for_each_shape_value(body, defs, &mut |v| {
        if let Some((lst, _)) = is_list_get(v) {
            if same_local(lst, xs, defs) {
                get = true;
            }
        }
        if matches!(v, Value::Binary { op: BinOp::Sub, .. }) {
            sub = true;
        }
        if matches!(v, Value::Binary { op: BinOp::Mul, .. }) {
            mul = true;
        }
        if matches!(v, Value::Binary { op: BinOp::Div, .. }) {
            div = true;
        }
        if is_list_set(v).is_some() {
            set = true;
        }
    });
    get && sub && mul && div && !set && body_calls_any(body, &["lumia_f64_sqrt", "sqrtF", "sqrt"])
}

fn fun_has_l2_normalize_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out: &OutSlot,
    xs: Local,
    eps: Local,
) -> bool {
    let mut get = false;
    let mut set = false;
    let mut mul = false;
    let mut uses_eps = false;
    for_each_shape_value(body, defs, &mut |v| {
        if let Some((lst, _)) = is_list_get(v) {
            if is_out_list(lst, out, xs, defs) {
                get = true;
            }
        }
        if is_out_set(v, out, xs, defs) {
            set = true;
        }
        if matches!(v, Value::Binary { op: BinOp::Mul, .. }) {
            mul = true;
        }
        if mentions_local(v, eps) {
            uses_eps = true;
        }
    });
    get && set && mul && uses_eps && body_calls_any(body, &["lumia_f64_sqrt", "sqrtF", "sqrt"])
}

fn fun_has_softmax_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out: &OutSlot,
    xs: Local,
) -> bool {
    let mut get = false;
    let mut set = false;
    let mut div = false;
    let mut gt = false;
    for_each_shape_value(body, defs, &mut |v| {
        if let Some((lst, _)) = is_list_get(v) {
            if is_out_list(lst, out, xs, defs) {
                get = true;
            }
        }
        if is_out_set(v, out, xs, defs) {
            set = true;
        }
        if matches!(v, Value::Binary { op: BinOp::Div, .. }) {
            div = true;
        }
        if matches!(v, Value::Binary { op: BinOp::Gt, .. }) {
            gt = true;
        }
        if matches!(v, Value::If { .. }) {
            // max-pass update often uses If
            gt = true;
        }
    });
    get && set && div && gt && body_calls_any(body, &["lumia_f64_exp", "expF", "exp"])
}
