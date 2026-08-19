use super::shape_util::{for_each_shape_value, is_out_list, is_out_set, mentions_local, OutSlot};
use lumia_core::CoreBinOp as BinOp;
use lumia_core::{
    for_each_assign_in_block, for_each_direct_loop_in_block, for_each_let_in_block,
    header_lt_bound, is_list_get, is_list_set, is_nontrivial_add_or_sub, is_unit_inc, same_local,
    Block, Local, Value,
};
use rustc_hash::FxHashMap as HashMap;

pub(super) fn body_has_gemv_inner(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out: &OutSlot,
    dest: Local,
    i_slot: &str,
    a: Local,
    x: Local,
    n: Local,
) -> bool {
    let mut saw_inner = false;
    let mut saw_set = false;
    let mut saw_i_inc = false;
    for_each_direct_loop_in_block(body, &mut |header, ib, latch| {
        if !latch.ops.is_empty() {
            return;
        }
        let Some((j_slot, bound)) = header_lt_bound(header, defs) else {
            return;
        };
        if !same_local(bound, n, defs) {
            return;
        }
        if gemv_inner_accumulates(ib, defs, &j_slot, a, x, n, i_slot) {
            saw_inner = true;
        }
    });
    for_each_assign_in_block(body, &mut |name, value| {
        if name == out.as_str() {
            if let Some(val) = defs.get(&value.0) {
                if is_out_set(val, out, dest, defs) {
                    saw_set = true;
                }
            }
        }
        if name == i_slot && is_unit_inc(value.0, i_slot, defs) {
            saw_i_inc = true;
        }
    });
    saw_inner && saw_set && saw_i_inc
}

fn gemv_inner_accumulates(
    body: &Block,
    defs: &HashMap<u32, Value>,
    j_slot: &str,
    a: Local,
    x: Local,
    n: Local,
    i_slot: &str,
) -> bool {
    let mut saw_mul_gets = false;
    let mut saw_j_inc = false;
    for_each_assign_in_block(body, &mut |name, value| {
        if name == j_slot && is_unit_inc(value.0, j_slot, defs) {
            saw_j_inc = true;
        }
    });
    for_each_let_in_block(body, &mut |_local, value, _pure| {
        if let Value::Binary {
            op: BinOp::Mul,
            left,
            right,
            ..
        } = value
        {
            let lg = defs.get(&left.0).and_then(is_list_get);
            let rg = defs.get(&right.0).and_then(is_list_get);
            if let (Some((la, _)), Some((lb, _))) = (lg, rg) {
                let a_x = (same_local(la, a, defs) && same_local(lb, x, defs))
                    || (same_local(la, x, defs) && same_local(lb, a, defs));
                if a_x {
                    let _ = (n, i_slot);
                    saw_mul_gets = true;
                }
            }
        }
    });
    saw_mul_gets && saw_j_inc
}

fn mul_gets_a_x(v: &Value, defs: &HashMap<u32, Value>, a: Local, x: Local) -> bool {
    let Value::Binary {
        op: BinOp::Mul,
        left,
        right,
        ..
    } = v
    else {
        return false;
    };
    let lg = defs.get(&left.0).and_then(is_list_get);
    let rg = defs.get(&right.0).and_then(is_list_get);
    let Some((la, _)) = lg else {
        return false;
    };
    let Some((lb, _)) = rg else {
        return false;
    };
    (same_local(la, a, defs) && same_local(lb, x, defs))
        || (same_local(la, x, defs) && same_local(lb, a, defs))
}

pub(super) fn fun_has_gemv_t_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out: &OutSlot,
    dest: Local,
    a: Local,
    x: Local,
    _m: Local,
    _n: Local,
) -> bool {
    let mut mul = false;
    let mut set = false;
    let mut zero_fill = false;
    for_each_shape_value(body, defs, &mut |v| {
        if mul_gets_a_x(v, defs, a, x) {
            mul = true;
        }
        if is_out_set(v, out, dest, defs) {
            set = true;
            if let Some((_, _, val)) = is_list_set(v) {
                if matches!(defs.get(&val.0), Some(Value::Float(f)) if *f == 0.0)
                    || matches!(defs.get(&val.0), Some(Value::Int(0)))
                {
                    zero_fill = true;
                }
            }
        }
    });
    mul && set && zero_fill
}

pub(super) fn fun_has_addmm_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out: &OutSlot,
    dest: Local,
    u: Local,
    v: Local,
    alpha: Local,
    _m: Local,
    _n: Local,
) -> bool {
    let mut get_u = false;
    let mut get_v = false;
    let mut set = false;
    let mut uses_alpha = false;
    for_each_shape_value(body, defs, &mut |vdef| {
        if let Some((lst, _)) = is_list_get(vdef) {
            if same_local(lst, u, defs) {
                get_u = true;
            }
            if same_local(lst, v, defs) {
                get_v = true;
            }
        }
        if is_out_set(vdef, out, dest, defs) {
            set = true;
        }
        if mentions_local(vdef, alpha) {
            uses_alpha = true;
        }
    });
    get_u && get_v && set && uses_alpha
}

pub(super) fn fun_has_axpy_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out: &OutSlot,
    dest: Local,
    x: Local,
    alpha: Local,
) -> bool {
    let mut get_x = false;
    let mut get_y = false;
    let mut set = false;
    let mut uses_alpha = false;
    for_each_shape_value(body, defs, &mut |v| {
        if let Some((lst, _)) = is_list_get(v) {
            if same_local(lst, x, defs) {
                get_x = true;
            }
            if is_out_list(lst, out, dest, defs) {
                get_y = true;
            }
        }
        if is_out_set(v, out, dest, defs) {
            set = true;
        }
        if mentions_local(v, alpha) {
            uses_alpha = true;
        }
    });
    get_x && get_y && set && uses_alpha
}

pub(super) fn fun_has_sub_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out: &OutSlot,
    dest: Local,
    a: Local,
    b: Local,
) -> bool {
    let mut get_a = false;
    let mut get_b = false;
    let mut sub = false;
    let mut set = false;
    for_each_shape_value(body, defs, &mut |v| {
        if let Some((lst, _)) = is_list_get(v) {
            if same_local(lst, a, defs) {
                get_a = true;
            }
            if same_local(lst, b, defs) {
                get_b = true;
            }
        }
        if matches!(v, Value::Binary { op: BinOp::Sub, .. }) {
            sub = true;
        }
        if is_out_set(v, out, dest, defs) {
            set = true;
        }
    });
    get_a && get_b && sub && set
}

pub(super) fn fun_has_add_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out: &OutSlot,
    dest: Local,
    a: Local,
    b: Local,
) -> bool {
    let mut get_a = false;
    let mut get_b = false;
    let mut add = false;
    let mut set = false;
    let mut mul = false;
    for_each_shape_value(body, defs, &mut |v| {
        if let Some((lst, _)) = is_list_get(v) {
            if same_local(lst, a, defs) || is_out_list(lst, out, dest, defs) {
                get_a = true;
            }
            if same_local(lst, b, defs) {
                get_b = true;
            }
        }
        if matches!(v, Value::Binary { op: BinOp::Add, .. }) {
            add = true;
        }
        if matches!(v, Value::Binary { op: BinOp::Mul, .. }) {
            mul = true;
        }
        if is_out_set(v, out, dest, defs) {
            set = true;
        }
    });
    // Exclude axpy-like `y + α*x` (has Mul).
    get_a && get_b && add && set && !mul
}

pub(super) fn fun_has_mul_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out: &OutSlot,
    dest: Local,
    a: Local,
    b: Local,
) -> bool {
    let mut get_a = false;
    let mut get_b = false;
    let mut mul = false;
    let mut set = false;
    let mut add_or_sub = false;
    for_each_shape_value(body, defs, &mut |v| {
        if let Some((lst, _)) = is_list_get(v) {
            if same_local(lst, a, defs) {
                get_a = true;
            }
            if same_local(lst, b, defs) {
                get_b = true;
            }
        }
        if matches!(v, Value::Binary { op: BinOp::Mul, .. }) {
            mul = true;
        }
        if is_nontrivial_add_or_sub(v, defs) {
            add_or_sub = true;
        }
        if is_out_set(v, out, dest, defs) {
            set = true;
        }
    });
    get_a && get_b && mul && set && !add_or_sub
}
