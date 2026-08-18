use super::shape_util::{
    first_assign_from_local, mentions_local, out_slot_for_list_param, param_float, param_int,
    param_list_f64, ret_list_f64,
};
use lumia_core::CoreBinOp as BinOp;
use lumia_core::{
    for_each_let_value_ctrl, header_lt_bound, is_list_get, is_list_set, is_nontrivial_add_or_sub,
    is_nontrivial_arith, name_of, same_local, Block, Local, Op, Value,
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
    let out_slot = out_slot_for_list_param(body, xs)?;
    if !fun_has_clamp_shape(body, defs, &out_slot, lo, hi) {
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
    let out_slot = out_slot_for_list_param(body, xs)?;
    if !fun_has_scale_shape(body, defs, &out_slot, xs, alpha) {
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
    let out_slot = out_slot_for_list_param(body, xs)?;
    if !fun_has_fill_shape(body, defs, &out_slot, v) {
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
    let out_slot = first_assign_from_local(body, dst)?;
    if !fun_has_copy_shape(body, defs, &out_slot, src) {
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
    // Must allocate a float list seed and append 0.0 in a loop bounded by n.
    let mut seed = false;
    let mut append0 = false;
    let mut bound_n = false;
    for v in defs.values() {
        if let Value::AllocList { elems, .. } = v {
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
        } = v
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
        } = v
        {
            if same_local(*right, n, defs) {
                bound_n = true;
            }
        }
    }
    for_each_let_value_ctrl(body, &mut |_b, val| {
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
    out_slot: &str,
    xs: Local,
    alpha: Local,
) -> bool {
    let mut get_y = false;
    let mut mul = false;
    let mut set = false;
    let mut uses_alpha = false;
    let mut add_or_sub = false;
    let is_out =
        |lst: Local| name_of(lst, defs).as_deref() == Some(out_slot) || same_local(lst, xs, defs);
    for v in defs.values() {
        if let Some((lst, _)) = is_list_get(v) {
            if is_out(lst) {
                get_y = true;
            }
        }
        if matches!(v, Value::Binary { op: BinOp::Mul, .. }) {
            mul = true;
        }
        if is_nontrivial_add_or_sub(v, defs) {
            add_or_sub = true;
        }
        if is_list_set(v).is_some() {
            set = true;
        }
        if mentions_local(v, alpha) {
            uses_alpha = true;
        }
    }
    for_each_let_value_ctrl(body, &mut |_b, val| {
        if is_list_set(val).is_some() {
            set = true;
        }
        if let Some((lst, _)) = is_list_get(val) {
            if is_out(lst) {
                get_y = true;
            }
        }
        if matches!(val, Value::Binary { op: BinOp::Mul, .. }) {
            mul = true;
        }
        if is_nontrivial_add_or_sub(val, defs) {
            add_or_sub = true;
        }
    });
    get_y && mul && set && uses_alpha && !add_or_sub
}

fn fun_has_fill_shape(body: &Block, defs: &HashMap<u32, Value>, out_slot: &str, v: Local) -> bool {
    let mut set = false;
    let mut uses_v = false;
    let mut get_any = false;
    let mut arith = false;
    for vdef in defs.values() {
        if let Some((_, _)) = is_list_get(vdef) {
            get_any = true;
        }
        if is_list_set(vdef).is_some() {
            set = true;
        }
        if mentions_local(vdef, v) {
            uses_v = true;
        }
        if is_nontrivial_arith(vdef, defs) {
            arith = true;
        }
    }
    for_each_let_value_ctrl(body, &mut |_b, val| {
        if is_list_set(val).is_some() {
            set = true;
        }
        if is_list_get(val).is_some() {
            get_any = true;
        }
        if is_nontrivial_arith(val, defs) {
            arith = true;
        }
    });
    let _ = out_slot;
    set && uses_v && !get_any && !arith
}

fn fun_has_clamp_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out_slot: &str,
    lo: Local,
    hi: Local,
) -> bool {
    let mut set = false;
    let mut uses_lo = false;
    let mut uses_hi = false;
    let mut saw_if = false;
    for_each_let_value_ctrl(body, &mut |_b, val| {
        if is_list_set(val).is_some() {
            set = true;
        }
        // Require a real `If` — loop `i < n` alone must not look like clamp.
        if matches!(val, Value::If { .. }) {
            saw_if = true;
        }
    });
    for v in defs.values() {
        if mentions_local(v, lo) {
            uses_lo = true;
        }
        if mentions_local(v, hi) {
            uses_hi = true;
        }
        if is_list_set(v).is_some() {
            set = true;
        }
    }
    for op in &body.ops {
        if let Op::Assign { name, .. } = op {
            if name == out_slot {
                set = true;
            }
        }
    }
    set && saw_if && uses_lo && uses_hi
}

fn fun_has_copy_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out_slot: &str,
    src: Local,
) -> bool {
    // out[i] = src[i]; no arithmetic on the transferred value.
    let mut get_src = false;
    let mut set = false;
    let mut saw_arith = false;
    for v in defs.values() {
        if let Some((lst, _)) = is_list_get(v) {
            if same_local(lst, src, defs) {
                get_src = true;
            }
        }
        if is_list_set(v).is_some() {
            set = true;
        }
        if matches!(
            v,
            Value::Binary {
                op: BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div,
                ..
            }
        ) {
            // Index `i*n+j` style shouldn't appear; len() compares are elsewhere.
            // Allow only if not feeding the set value — soft: any Mul/Div is suspicious.
            if matches!(
                v,
                Value::Binary {
                    op: BinOp::Mul | BinOp::Div | BinOp::Sub,
                    ..
                }
            ) {
                saw_arith = true;
            }
        }
    }
    for_each_let_value_ctrl(body, &mut |_b, val| {
        if is_list_set(val).is_some() {
            set = true;
        }
        if let Some((lst, _)) = is_list_get(val) {
            if same_local(lst, src, defs) {
                get_src = true;
            }
        }
    });
    let _ = out_slot;
    get_src && set && !saw_arith
}
