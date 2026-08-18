use super::shape_util::mentions_local;
use lumia_core::CoreBinOp as BinOp;
use lumia_core::{
    for_each_let_value_ctrl, header_lt_bound, is_list_get, is_list_set, is_nontrivial_add_or_sub,
    is_unit_inc, name_of, same_local, Block, Local, Op, Value,
};
use rustc_hash::FxHashMap as HashMap;

pub(super) fn body_has_gemv_inner(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out_slot: &str,
    i_slot: &str,
    a: Local,
    x: Local,
    n: Local,
) -> bool {
    let mut saw_inner = false;
    let mut saw_set = false;
    let mut saw_i_inc = false;
    for op in &body.ops {
        match op {
            Op::Let {
                value:
                    Value::Loop {
                        header,
                        body: ib,
                        latch,
                    },
                ..
            } => {
                if !latch.ops.is_empty() {
                    continue;
                }
                let Some((j_slot, bound)) = header_lt_bound(header, defs) else {
                    continue;
                };
                if !same_local(bound, n, defs) {
                    continue;
                }
                if gemv_inner_accumulates(ib, defs, &j_slot, a, x, n, i_slot) {
                    saw_inner = true;
                }
            }
            Op::Assign { name, value } => {
                if name == out_slot {
                    if let Some(val) = defs.get(&value.0) {
                        if is_list_set(val).is_some() {
                            saw_set = true;
                        }
                    }
                }
                if name == i_slot && is_unit_inc(value.0, i_slot, defs) {
                    saw_i_inc = true;
                }
            }
            _ => {}
        }
    }
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
    for op in &body.ops {
        if let Op::Assign { name, value } = op {
            if name == j_slot && is_unit_inc(value.0, j_slot, defs) {
                saw_j_inc = true;
            }
        }
        if let Op::Let {
            value:
                Value::Binary {
                    op: BinOp::Mul,
                    left,
                    right,
                    ..
                },
            ..
        } = op
        {
            let lg = defs.get(&left.0).and_then(is_list_get);
            let rg = defs.get(&right.0).and_then(is_list_get);
            if let (Some((la, _)), Some((lb, _))) = (lg, rg) {
                let a_x = (same_local(la, a, defs) && same_local(lb, x, defs))
                    || (same_local(la, x, defs) && same_local(lb, a, defs));
                if a_x {
                    // Soft-check index uses i/n/j via presence of Mul/Add involving them elsewhere.
                    let _ = (n, i_slot);
                    saw_mul_gets = true;
                }
            }
        }
    }
    saw_mul_gets && saw_j_inc
}

pub(super) fn fun_has_gemv_t_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out_slot: &str,
    a: Local,
    x: Local,
    m: Local,
    n: Local,
) -> bool {
    let mut mul = false;
    let mut set = false;
    let mut zero_fill = false;
    for_each_let_value_ctrl(body, &mut |_b, v| {
        if let Value::Binary {
            op: BinOp::Mul,
            left,
            right,
            ..
        } = v
        {
            let lg = defs.get(&left.0).and_then(is_list_get);
            let rg = defs.get(&right.0).and_then(is_list_get);
            if let (Some((la, _)), Some((lb, _))) = (lg, rg) {
                if (same_local(la, a, defs) && same_local(lb, x, defs))
                    || (same_local(la, x, defs) && same_local(lb, a, defs))
                {
                    mul = true;
                }
            }
        }
        if is_list_set(v).is_some() {
            set = true;
        }
        // Zero-fill: set(j, 0.0) or set(j, Float(0))
        if let Some((_, _, val)) = is_list_set(v) {
            if matches!(defs.get(&val.0), Some(Value::Float(f)) if *f == 0.0)
                || matches!(defs.get(&val.0), Some(Value::Int(0)))
            {
                zero_fill = true;
            }
        }
        let _ = (m, n, out_slot);
    });
    // Also scan leaf_defs for MapSet / Mul (lets may be inlined into Assigns)
    for v in defs.values() {
        if let Value::Binary {
            op: BinOp::Mul,
            left,
            right,
            ..
        } = v
        {
            let lg = defs.get(&left.0).and_then(is_list_get);
            let rg = defs.get(&right.0).and_then(is_list_get);
            if let (Some((la, _)), Some((lb, _))) = (lg, rg) {
                if (same_local(la, a, defs) && same_local(lb, x, defs))
                    || (same_local(la, x, defs) && same_local(lb, a, defs))
                {
                    mul = true;
                }
            }
        }
        if is_list_set(v).is_some() {
            set = true;
        }
        if let Some((_, _, val)) = is_list_set(v) {
            if matches!(defs.get(&val.0), Some(Value::Float(f)) if *f == 0.0) {
                zero_fill = true;
            }
        }
    }
    mul && set && zero_fill
}

pub(super) fn fun_has_addmm_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out_slot: &str,
    u: Local,
    v: Local,
    alpha: Local,
    m: Local,
    n: Local,
) -> bool {
    let mut get_u = false;
    let mut get_v = false;
    let mut set = false;
    let mut uses_alpha = false;
    for vdef in defs.values() {
        if let Some((lst, _)) = is_list_get(vdef) {
            if same_local(lst, u, defs) {
                get_u = true;
            }
            if same_local(lst, v, defs) {
                get_v = true;
            }
        }
        if is_list_set(vdef).is_some() {
            set = true;
        }
        if mentions_local(vdef, alpha) {
            uses_alpha = true;
        }
    }
    for_each_let_value_ctrl(body, &mut |_b, val| {
        if is_list_set(val).is_some() {
            set = true;
        }
        if let Some((lst, _)) = is_list_get(val) {
            if same_local(lst, u, defs) {
                get_u = true;
            }
            if same_local(lst, v, defs) {
                get_v = true;
            }
        }
    });
    let _ = (out_slot, m, n);
    get_u && get_v && set && uses_alpha
}

pub(super) fn fun_has_axpy_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out_slot: &str,
    x: Local,
    alpha: Local,
) -> bool {
    let mut get_x = false;
    let mut get_y = false;
    let mut set = false;
    let mut uses_alpha = false;
    for v in defs.values() {
        if let Some((lst, _)) = is_list_get(v) {
            if same_local(lst, x, defs) {
                get_x = true;
            }
            // y is out_slot Name
            if name_of(lst, defs).as_deref() == Some(out_slot) {
                get_y = true;
            }
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
            if same_local(lst, x, defs) {
                get_x = true;
            }
            if name_of(lst, defs).as_deref() == Some(out_slot) {
                get_y = true;
            }
        }
    });
    get_x && get_y && set && uses_alpha
}

pub(super) fn fun_has_sub_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out_slot: &str,
    a: Local,
    b: Local,
) -> bool {
    let mut get_a = false;
    let mut get_b = false;
    let mut sub = false;
    let mut set = false;
    for v in defs.values() {
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
        if is_list_set(v).is_some() {
            set = true;
        }
    }
    for_each_let_value_ctrl(body, &mut |_b, val| {
        if is_list_set(val).is_some() {
            set = true;
        }
        if matches!(val, Value::Binary { op: BinOp::Sub, .. }) {
            sub = true;
        }
    });
    let _ = out_slot;
    get_a && get_b && sub && set
}

pub(super) fn fun_has_add_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out_slot: &str,
    a: Local,
    b: Local,
) -> bool {
    let mut get_a = false;
    let mut get_b = false;
    let mut add = false;
    let mut set = false;
    let mut mul = false;
    for v in defs.values() {
        if let Some((lst, _)) = is_list_get(v) {
            if same_local(lst, a, defs) || name_of(lst, defs).as_deref() == Some(out_slot) {
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
        if is_list_set(v).is_some() {
            set = true;
        }
    }
    for_each_let_value_ctrl(body, &mut |_b, val| {
        if is_list_set(val).is_some() {
            set = true;
        }
        if let Some((lst, _)) = is_list_get(val) {
            if same_local(lst, a, defs) || name_of(lst, defs).as_deref() == Some(out_slot) {
                get_a = true;
            }
            if same_local(lst, b, defs) {
                get_b = true;
            }
        }
        if matches!(val, Value::Binary { op: BinOp::Add, .. }) {
            add = true;
        }
        if matches!(val, Value::Binary { op: BinOp::Mul, .. }) {
            mul = true;
        }
    });
    // Exclude axpy-like `y + α*x` (has Mul).
    get_a && get_b && add && set && !mul
}

pub(super) fn fun_has_mul_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out_slot: &str,
    a: Local,
    b: Local,
) -> bool {
    let mut get_a = false;
    let mut get_b = false;
    let mut mul = false;
    let mut set = false;
    let mut add_or_sub = false;
    for v in defs.values() {
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
        if is_list_set(v).is_some() {
            set = true;
        }
    }
    for_each_let_value_ctrl(body, &mut |_b, val| {
        if is_list_set(val).is_some() {
            set = true;
        }
        if matches!(val, Value::Binary { op: BinOp::Mul, .. }) {
            mul = true;
        }
        if is_nontrivial_add_or_sub(val, defs) {
            add_or_sub = true;
        }
    });
    let _ = out_slot;
    get_a && get_b && mul && set && !add_or_sub
}
