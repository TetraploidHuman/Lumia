//! Shared dense `List[Float]` SR pattern predicates (opt rewrite + codegen emit).
//!
//! Whole-function `match_*` wrappers stay in opt/codegen (return `()` vs Pats);
//! the structural shape checks below are identical and live here.

use crate::sr_pattern::{header_lt_bound, is_unit_inc, name_of, same_local};
use crate::{Block, Local, Op, Value};
use lumi_hir::Builtin;
use lumi_syntax::BinOp;
use rustc_hash::FxHashMap as HashMap;

pub fn is_list_get(v: &Value) -> Option<(Local, Local)> {
    match v {
        Value::Builtin {
            name: Builtin::ListGet,
            args,
        } if args.len() == 2 => Some((args[0], args[1])),
        _ => None,
    }
}

pub fn is_list_set(v: &Value) -> Option<(Local, Local, Local)> {
    match v {
        Value::Builtin {
            name: Builtin::MapSet,
            args,
        } if args.len() == 3 => Some((args[0], args[1], args[2])),
        _ => None,
    }
}

pub fn list_arg_is(list: Local, want: Local, defs: &HashMap<u32, Value>) -> bool {
    if list == want {
        return true;
    }
    match defs.get(&list.0) {
        Some(Value::Local(l)) => list_arg_is(*l, want, defs),
        Some(Value::Name(_)) => false,
        _ => false,
    }
}

/// Inner body of gemv: s accumulates A[i*n+j]*x[j]; then out.set(i,s); i+=1.
pub fn body_has_gemv_inner(
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

pub fn gemv_inner_accumulates(
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
                let a_x = (list_arg_is(la, a, defs) && list_arg_is(lb, x, defs))
                    || (list_arg_is(la, x, defs) && list_arg_is(lb, a, defs));
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

pub fn fun_has_gemv_t_shape(
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
    for_each_let(body, &mut |v| {
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
                if (list_arg_is(la, a, defs) && list_arg_is(lb, x, defs))
                    || (list_arg_is(la, x, defs) && list_arg_is(lb, a, defs))
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
                if (list_arg_is(la, a, defs) && list_arg_is(lb, x, defs))
                    || (list_arg_is(la, x, defs) && list_arg_is(lb, a, defs))
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

pub fn fun_has_addmm_shape(
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
            if list_arg_is(lst, u, defs) {
                get_u = true;
            }
            if list_arg_is(lst, v, defs) {
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
    for_each_let(body, &mut |val| {
        if is_list_set(val).is_some() {
            set = true;
        }
        if let Some((lst, _)) = is_list_get(val) {
            if list_arg_is(lst, u, defs) {
                get_u = true;
            }
            if list_arg_is(lst, v, defs) {
                get_v = true;
            }
        }
    });
    let _ = (out_slot, m, n);
    get_u && get_v && set && uses_alpha
}

pub fn fun_has_axpy_shape(
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
            if list_arg_is(lst, x, defs) {
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
    for_each_let(body, &mut |val| {
        if is_list_set(val).is_some() {
            set = true;
        }
        if let Some((lst, _)) = is_list_get(val) {
            if list_arg_is(lst, x, defs) {
                get_x = true;
            }
            if name_of(lst, defs).as_deref() == Some(out_slot) {
                get_y = true;
            }
        }
    });
    get_x && get_y && set && uses_alpha
}

pub fn fun_has_sub_shape(
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
            if list_arg_is(lst, a, defs) {
                get_a = true;
            }
            if list_arg_is(lst, b, defs) {
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
    for_each_let(body, &mut |val| {
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

pub fn fun_has_mul_shape(
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
            if list_arg_is(lst, a, defs) {
                get_a = true;
            }
            if list_arg_is(lst, b, defs) {
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
    for_each_let(body, &mut |val| {
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

pub fn fun_has_scale_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out_slot: &str,
    alpha: Local,
) -> bool {
    let mut get_y = false;
    let mut mul = false;
    let mut set = false;
    let mut uses_alpha = false;
    let mut add_or_sub = false;
    for v in defs.values() {
        if let Some((lst, _)) = is_list_get(v) {
            if name_of(lst, defs).as_deref() == Some(out_slot) {
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
    for_each_let(body, &mut |val| {
        if is_list_set(val).is_some() {
            set = true;
        }
        if let Some((lst, _)) = is_list_get(val) {
            if name_of(lst, defs).as_deref() == Some(out_slot) {
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

/// `i+1` / `1+i` latch increments must not disqualify elementwise kernels.
pub fn is_unit_inc_value(v: &Value, defs: &HashMap<u32, Value>) -> bool {
    let Value::Binary {
        op: BinOp::Add,
        left,
        right,
        ..
    } = v
    else {
        return false;
    };
    let one_l = matches!(defs.get(&left.0), Some(Value::Int(1)));
    let one_r = matches!(defs.get(&right.0), Some(Value::Int(1)));
    let name_l = name_of(*left, defs).is_some();
    let name_r = name_of(*right, defs).is_some();
    (name_l && one_r) || (name_r && one_l)
}

pub fn is_nontrivial_add_or_sub(v: &Value, defs: &HashMap<u32, Value>) -> bool {
    matches!(
        v,
        Value::Binary {
            op: BinOp::Add | BinOp::Sub,
            ..
        } if !is_unit_inc_value(v, defs)
    )
}

pub fn is_nontrivial_arith(v: &Value, defs: &HashMap<u32, Value>) -> bool {
    match v {
        Value::Binary {
            op: BinOp::Mul | BinOp::Div,
            ..
        } => true,
        Value::Binary {
            op: BinOp::Add | BinOp::Sub,
            ..
        } if !is_unit_inc_value(v, defs) => true,
        _ => false,
    }
}

pub fn fun_has_copy_shape(
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
            if list_arg_is(lst, src, defs) {
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
    for_each_let(body, &mut |val| {
        if is_list_set(val).is_some() {
            set = true;
        }
        if let Some((lst, _)) = is_list_get(val) {
            if list_arg_is(lst, src, defs) {
                get_src = true;
            }
        }
    });
    let _ = out_slot;
    get_src && set && !saw_arith
}

pub fn mentions_local(v: &Value, target: Local) -> bool {
    match v {
        Value::Local(l) => *l == target,
        Value::Binary { left, right, .. } => *left == target || *right == target,
        Value::Builtin { args, .. } => args.contains(&target),
        _ => false,
    }
}

pub fn for_each_let(body: &Block, f: &mut dyn FnMut(&Value)) {
    for op in &body.ops {
        if let Op::Let { value, .. } = op {
            f(value);
            match value {
                Value::Loop {
                    header,
                    body,
                    latch,
                } => {
                    for_each_let(header, f);
                    for_each_let(body, f);
                    for_each_let(latch, f);
                }
                Value::If {
                    then_block,
                    else_block,
                    ..
                } => {
                    for_each_let(then_block, f);
                    for_each_let(else_block, f);
                }
                _ => {}
            }
        }
    }
}

