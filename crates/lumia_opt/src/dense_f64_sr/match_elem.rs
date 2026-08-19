use super::shape_util::{
    for_each_shape_value, is_out_list, is_out_set, mentions_local, out_slot_for_list_param,
    param_float, param_int, param_list_f64, ret_list_f64, OutSlot,
};
use lumia_core::CoreBinOp as BinOp;
use lumia_core::{
    header_lt_bound, is_list_get, is_nontrivial_add_or_sub, is_nontrivial_arith, same_local, Block,
    Local, Value,
};
use lumia_hir::Builtin;
use rustc_hash::FxHashMap as HashMap;

pub(super) fn match_clamp_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    // Require List[Float] + Float bounds — bare arity/shape matched Int loops
    // (e.g. `collatzStrided`) and rewrote them to `lumia_f64_clamp`.
    if fun.params.len() != 3
        || !param_list_f64(fun, 0)
        || !param_float(fun, 1)
        || !param_float(fun, 2)
        || !ret_list_f64(fun)
    {
        return None;
    }
    let (xs, lo, hi) = (fun.params[0], fun.params[1], fun.params[2]);
    let body = &fun.body;
    let out = out_slot_for_list_param(body, xs)?;
    if !fun_has_clamp_shape(body, defs, &out, xs, lo, hi) {
        return None;
    }
    Some(())
}

pub(super) fn match_scale_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 2
        || !param_list_f64(fun, 0)
        || !param_float(fun, 1)
        || !ret_list_f64(fun)
    {
        return None;
    }
    let (xs, alpha) = (fun.params[0], fun.params[1]);
    let body = &fun.body;
    let out = out_slot_for_list_param(body, xs)?;
    if !fun_has_scale_shape(body, defs, &out, xs, alpha) {
        return None;
    }
    Some(())
}

pub(super) fn match_fill_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 2
        || !param_list_f64(fun, 0)
        || !param_float(fun, 1)
        || !ret_list_f64(fun)
    {
        return None;
    }
    let (xs, v) = (fun.params[0], fun.params[1]);
    let body = &fun.body;
    let out = out_slot_for_list_param(body, xs)?;
    if !fun_has_fill_shape(body, defs, &out, xs, v) {
        return None;
    }
    Some(())
}

pub(super) fn match_copy_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 2
        || !param_list_f64(fun, 0)
        || !param_list_f64(fun, 1)
        || !ret_list_f64(fun)
    {
        return None;
    }
    let (dst, src) = (fun.params[0], fun.params[1]);
    let body = &fun.body;
    let out = out_slot_for_list_param(body, dst)?;
    if !fun_has_copy_shape(body, defs, &out, dst, src) {
        return None;
    }
    Some(())
}

pub(super) fn match_zeros_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 1 || !param_int(fun, 0) || !ret_list_f64(fun) {
        return None;
    }
    let n = fun.params[0];
    let body = &fun.body;
    let mut seed = false;
    let mut append0 = false;
    let mut bound_n = false;
    for_each_shape_value(body, defs, &mut |val| {
        if let Value::AllocList { elems, .. } = val {
            if elems.len() <= 1
                && elems
                    .iter()
                    .all(|e| matches!(defs.get(&e.0), Some(Value::Float(f)) if *f == 0.0))
            {
                seed = true;
            }
        }
        if let Value::Builtin {
            name: Builtin::ListAppend,
            args,
            ..
        } = val
        {
            if args.len() == 2 && matches!(defs.get(&args[1].0), Some(Value::Float(f)) if *f == 0.0)
            {
                append0 = true;
            }
        }
        if let Value::Binary {
            op: BinOp::Lt,
            right,
            ..
        } = val
        {
            if same_local(*right, n, defs) {
                bound_n = true;
            }
        }
        if let Value::Loop { header, .. } = val {
            if let Some((_, bound)) = header_lt_bound(header, defs) {
                if same_local(bound, n, defs) {
                    bound_n = true;
                }
            }
        }
    });
    if seed && append0 && bound_n {
        Some(())
    } else {
        None
    }
}

fn fun_has_scale_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out: &OutSlot,
    xs: Local,
    alpha: Local,
) -> bool {
    let mut get_y = false;
    let mut mul = false;
    let mut set = false;
    let mut uses_alpha = false;
    let mut add_or_sub = false;
    for_each_shape_value(body, defs, &mut |v| {
        if let Some((lst, _)) = is_list_get(v) {
            if is_out_list(lst, out, xs, defs) {
                get_y = true;
            }
        }
        if matches!(v, Value::Binary { op: BinOp::Mul, .. }) {
            mul = true;
        }
        if is_nontrivial_add_or_sub(v, defs) {
            add_or_sub = true;
        }
        if is_out_set(v, out, xs, defs) {
            set = true;
        }
        if mentions_local(v, alpha) {
            uses_alpha = true;
        }
    });
    get_y && mul && set && uses_alpha && !add_or_sub
}

fn fun_has_fill_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out: &OutSlot,
    xs: Local,
    v: Local,
) -> bool {
    let mut set = false;
    let mut uses_v = false;
    let mut get_any = false;
    let mut arith = false;
    for_each_shape_value(body, defs, &mut |vdef| {
        if is_list_get(vdef).is_some() {
            get_any = true;
        }
        if is_out_set(vdef, out, xs, defs) {
            set = true;
        }
        if mentions_local(vdef, v) {
            uses_v = true;
        }
        if is_nontrivial_arith(vdef, defs) {
            arith = true;
        }
    });
    set && uses_v && !get_any && !arith
}

fn fun_has_clamp_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out: &OutSlot,
    xs: Local,
    lo: Local,
    hi: Local,
) -> bool {
    let mut set = false;
    let mut uses_lo = false;
    let mut uses_hi = false;
    let mut saw_if = false;
    for_each_shape_value(body, defs, &mut |val| {
        if is_out_set(val, out, xs, defs) {
            set = true;
        }
        // Require a real `If` — loop `i < n` alone must not look like clamp.
        if matches!(val, Value::If { .. }) {
            saw_if = true;
        }
        if mentions_local(val, lo) {
            uses_lo = true;
        }
        if mentions_local(val, hi) {
            uses_hi = true;
        }
    });
    set && saw_if && uses_lo && uses_hi
}

fn fun_has_copy_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out: &OutSlot,
    dst: Local,
    src: Local,
) -> bool {
    // out[i] = src[i]; no arithmetic on the transferred value.
    let mut get_src = false;
    let mut set = false;
    let mut saw_arith = false;
    for_each_shape_value(body, defs, &mut |v| {
        if let Some((lst, _)) = is_list_get(v) {
            if same_local(lst, src, defs) {
                get_src = true;
            }
        }
        if is_out_set(v, out, dst, defs) {
            set = true;
        }
        if matches!(
            v,
            Value::Binary {
                op: BinOp::Mul | BinOp::Div | BinOp::Sub,
                ..
            }
        ) {
            // Index `i*n+j` style shouldn't appear; len() compares are elsewhere.
            saw_arith = true;
        }
    });
    get_src && set && !saw_arith
}
