//! Shared dense `List[Float]` SR pattern matchers (opt rewrite + codegen emit).
//!
//! Whole-function `match_*` return param bundles; opt maps to `()` / symbol,
//! codegen uses the same structs for RT kernel emission.

use crate::sr_pattern::{
    first_assign_from_local, first_loop, header_lt_bound, is_unit_inc, name_of, same_local,
};
use crate::{Block, CoreFun, Local, Op, Value};
use lumi_hir::Builtin;
use lumi_syntax::BinOp;
use lumi_ty::Type;
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
    for_each_def_and_let(body, defs, &mut |v| {
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
        if let Some((_, _, val)) = is_list_set(v) {
            set = true;
            if matches!(defs.get(&val.0), Some(Value::Float(f)) if *f == 0.0)
                || matches!(defs.get(&val.0), Some(Value::Int(0)))
            {
                zero_fill = true;
            }
        }
        let _ = (m, n, out_slot);
    });
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
    for_each_def_and_let(body, defs, &mut |vdef| {
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
    for_each_def_and_let(body, defs, &mut |v| {
        if let Some((lst, _)) = is_list_get(v) {
            if list_arg_is(lst, x, defs) {
                get_x = true;
            }
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
    });
    get_x && get_y && set && uses_alpha
}

#[derive(Clone, Copy)]
struct Bin3ShapeFlags {
    require_op: BinOp,
    forbid_mul: bool,
    forbid_nontrivial_add_sub: bool,
    out_slot_as_get_a: bool,
}

fn fun_has_bin3_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out_slot: &str,
    a: Local,
    b: Local,
    flags: Bin3ShapeFlags,
) -> bool {
    let mut get_a = false;
    let mut get_b = false;
    let mut has_op = false;
    let mut set = false;
    let mut has_mul = false;
    let mut add_or_sub = false;
    for_each_def_and_let(body, defs, &mut |v| {
        if let Some((lst, _)) = is_list_get(v) {
            if list_arg_is(lst, a, defs)
                || (flags.out_slot_as_get_a && name_of(lst, defs).as_deref() == Some(out_slot))
            {
                get_a = true;
            }
            if list_arg_is(lst, b, defs) {
                get_b = true;
            }
        }
        if matches!(v, Value::Binary { op, .. } if *op == flags.require_op) {
            has_op = true;
        }
        if matches!(v, Value::Binary { op: BinOp::Mul, .. }) {
            has_mul = true;
        }
        if is_nontrivial_add_or_sub(v, defs) {
            add_or_sub = true;
        }
        if is_list_set(v).is_some() {
            set = true;
        }
    });
    let _ = out_slot;
    get_a
        && get_b
        && has_op
        && set
        && (!flags.forbid_mul || !has_mul)
        && (!flags.forbid_nontrivial_add_sub || !add_or_sub)
}

pub fn fun_has_sub_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out_slot: &str,
    a: Local,
    b: Local,
) -> bool {
    fun_has_bin3_shape(
        body,
        defs,
        out_slot,
        a,
        b,
        Bin3ShapeFlags {
            require_op: BinOp::Sub,
            forbid_mul: false,
            forbid_nontrivial_add_sub: false,
            out_slot_as_get_a: false,
        },
    )
}

pub fn fun_has_mul_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out_slot: &str,
    a: Local,
    b: Local,
) -> bool {
    fun_has_bin3_shape(
        body,
        defs,
        out_slot,
        a,
        b,
        Bin3ShapeFlags {
            require_op: BinOp::Mul,
            forbid_mul: false,
            forbid_nontrivial_add_sub: true,
            out_slot_as_get_a: false,
        },
    )
}

pub fn fun_has_add_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out_slot: &str,
    a: Local,
    b: Local,
) -> bool {
    fun_has_bin3_shape(
        body,
        defs,
        out_slot,
        a,
        b,
        Bin3ShapeFlags {
            require_op: BinOp::Add,
            forbid_mul: true,
            forbid_nontrivial_add_sub: false,
            out_slot_as_get_a: true,
        },
    )
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
    for_each_def_and_let(body, defs, &mut |v| {
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
    for_each_def_and_let(body, defs, &mut |v| {
        if let Some((lst, _)) = is_list_get(v) {
            if list_arg_is(lst, src, defs) {
                get_src = true;
            }
        }
        if is_list_set(v).is_some() {
            set = true;
        }
        // Index `i*n+j` style shouldn't appear; soft: any Mul/Div/Sub is suspicious.
        if matches!(
            v,
            Value::Binary {
                op: BinOp::Mul | BinOp::Div | BinOp::Sub,
                ..
            }
        ) {
            saw_arith = true;
        }
    });
    let _ = out_slot;
    get_src && set && !saw_arith
}

pub fn fun_has_fill_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out_slot: &str,
    v: Local,
) -> bool {
    let mut set = false;
    let mut uses_v = false;
    let mut get_any = false;
    let mut arith = false;
    for_each_def_and_let(body, defs, &mut |val| {
        if is_list_get(val).is_some() {
            get_any = true;
        }
        if is_list_set(val).is_some() {
            set = true;
        }
        if mentions_local(val, v) {
            uses_v = true;
        }
        if is_nontrivial_arith(val, defs) {
            arith = true;
        }
    });
    let _ = out_slot;
    set && uses_v && !get_any && !arith
}

pub fn fun_has_clamp_shape(
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
    for_each_def_and_let(body, defs, &mut |v| {
        if is_list_set(v).is_some() {
            set = true;
        }
        if mentions_local(v, lo) {
            uses_lo = true;
        }
        if mentions_local(v, hi) {
            uses_hi = true;
        }
        // Require a real `If` — loop `i < n` alone must not look like clamp.
        if matches!(v, Value::If { .. }) {
            saw_if = true;
        }
    });
    for op in &body.ops {
        if let Op::Assign { name, .. } = op {
            if name == out_slot {
                set = true;
            }
        }
    }
    set && saw_if && uses_lo && uses_hi
}

#[derive(Debug, Clone, Copy)]
pub struct DenseGemv {
    pub m: Local,
    pub n: Local,
    pub a: Local,
    pub x: Local,
    pub y: Local,
}

#[derive(Debug, Clone, Copy)]
pub struct DenseAddmm {
    pub m: Local,
    pub n: Local,
    pub w: Local,
    pub u: Local,
    pub v: Local,
    pub alpha: Local,
}

#[derive(Debug, Clone, Copy)]
pub struct DenseAxpy {
    pub y: Local,
    pub alpha: Local,
    pub x: Local,
}

#[derive(Debug, Clone, Copy)]
pub struct DenseBin3 {
    pub out: Local,
    pub a: Local,
    pub b: Local,
}

#[derive(Debug, Clone, Copy)]
pub struct DenseClamp {
    pub xs: Local,
    pub lo: Local,
    pub hi: Local,
}

#[derive(Debug, Clone, Copy)]
pub struct DenseScale {
    pub xs: Local,
    pub alpha: Local,
}

#[derive(Debug, Clone, Copy)]
pub struct DenseFill {
    pub xs: Local,
    pub v: Local,
}

#[derive(Debug, Clone, Copy)]
pub struct DenseCopy {
    pub dst: Local,
    pub src: Local,
}

pub fn match_gemv_fun(fun: &CoreFun, defs: &HashMap<u32, Value>) -> Option<DenseGemv> {
    if fun.params.len() != 5 {
        return None;
    }
    let (m, n, a, x, y) = (
        fun.params[0],
        fun.params[1],
        fun.params[2],
        fun.params[3],
        fun.params[4],
    );
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, y)?;
    let (header, loop_body, latch) = first_loop(body)?;
    if !latch.ops.is_empty() {
        return None;
    }
    let (i_slot, bound) = header_lt_bound(header, defs)?;
    if !same_local(bound, m, defs) {
        return None;
    }
    if !body_has_gemv_inner(loop_body, defs, &out_slot, &i_slot, a, x, n) {
        return None;
    }
    Some(DenseGemv { m, n, a, x, y })
}

pub fn match_gemv_t_fun(fun: &CoreFun, defs: &HashMap<u32, Value>) -> Option<DenseGemv> {
    if fun.params.len() != 5 {
        return None;
    }
    let (m, n, a, x, y) = (
        fun.params[0],
        fun.params[1],
        fun.params[2],
        fun.params[3],
        fun.params[4],
    );
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, y)?;
    if !fun_has_gemv_t_shape(body, defs, &out_slot, a, x, m, n) {
        return None;
    }
    Some(DenseGemv { m, n, a, x, y })
}

pub fn match_addmm_fun(fun: &CoreFun, defs: &HashMap<u32, Value>) -> Option<DenseAddmm> {
    if fun.params.len() != 6 {
        return None;
    }
    let (m, n, w, u, v, alpha) = (
        fun.params[0],
        fun.params[1],
        fun.params[2],
        fun.params[3],
        fun.params[4],
        fun.params[5],
    );
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, w)?;
    if !fun_has_addmm_shape(body, defs, &out_slot, u, v, alpha, m, n) {
        return None;
    }
    Some(DenseAddmm {
        m,
        n,
        w,
        u,
        v,
        alpha,
    })
}

pub fn match_axpy_fun(fun: &CoreFun, defs: &HashMap<u32, Value>) -> Option<DenseAxpy> {
    if fun.params.len() != 3 {
        return None;
    }
    let (y, alpha, x) = (fun.params[0], fun.params[1], fun.params[2]);
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, y)?;
    if !fun_has_axpy_shape(body, defs, &out_slot, x, alpha) {
        return None;
    }
    Some(DenseAxpy { y, alpha, x })
}

fn match_bin3_fun(
    fun: &CoreFun,
    defs: &HashMap<u32, Value>,
    shape: fn(&Block, &HashMap<u32, Value>, &str, Local, Local) -> bool,
) -> Option<DenseBin3> {
    if fun.params.len() != 3 {
        return None;
    }
    let (out, a, b) = (fun.params[0], fun.params[1], fun.params[2]);
    let out_slot = first_assign_from_local(&fun.body, out)?;
    if !shape(&fun.body, defs, &out_slot, a, b) {
        return None;
    }
    Some(DenseBin3 { out, a, b })
}

pub fn match_sub_fun(fun: &CoreFun, defs: &HashMap<u32, Value>) -> Option<DenseBin3> {
    match_bin3_fun(fun, defs, fun_has_sub_shape)
}

pub fn match_add_fun(fun: &CoreFun, defs: &HashMap<u32, Value>) -> Option<DenseBin3> {
    match_bin3_fun(fun, defs, fun_has_add_shape)
}

pub fn match_mul_fun(fun: &CoreFun, defs: &HashMap<u32, Value>) -> Option<DenseBin3> {
    match_bin3_fun(fun, defs, fun_has_mul_shape)
}

pub fn match_clamp_fun(fun: &CoreFun, defs: &HashMap<u32, Value>) -> Option<DenseClamp> {
    if fun.params.len() != 3 {
        return None;
    }
    let (xs, lo, hi) = (fun.params[0], fun.params[1], fun.params[2]);
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, xs)?;
    if !fun_has_clamp_shape(body, defs, &out_slot, lo, hi) {
        return None;
    }
    Some(DenseClamp { xs, lo, hi })
}

pub fn match_scale_fun(fun: &CoreFun, defs: &HashMap<u32, Value>) -> Option<DenseScale> {
    if fun.params.len() != 2 {
        return None;
    }
    let (xs, alpha) = (fun.params[0], fun.params[1]);
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, xs)?;
    if !fun_has_scale_shape(body, defs, &out_slot, alpha) {
        return None;
    }
    Some(DenseScale { xs, alpha })
}

pub fn match_fill_fun(fun: &CoreFun, defs: &HashMap<u32, Value>) -> Option<DenseFill> {
    if fun.params.len() != 2 {
        return None;
    }
    let (xs, v) = (fun.params[0], fun.params[1]);
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, xs)?;
    if !fun_has_fill_shape(body, defs, &out_slot, v) {
        return None;
    }
    Some(DenseFill { xs, v })
}

pub fn match_copy_fun(fun: &CoreFun, defs: &HashMap<u32, Value>) -> Option<DenseCopy> {
    if fun.params.len() != 2 {
        return None;
    }
    let (dst, src) = (fun.params[0], fun.params[1]);
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, dst)?;
    if !fun_has_copy_shape(body, defs, &out_slot, src) {
        return None;
    }
    Some(DenseCopy { dst, src })
}

fn body_calls_any(body: &Block, names: &[&str]) -> bool {
    let mut found = false;
    crate::visit::for_each_let(body, &mut |val| {
        if let Value::Call { fun, .. } = val {
            if names.iter().any(|n| fun == n) {
                found = true;
            }
        }
    });
    found
}

fn fun_has_sum_sq_shape(body: &Block, defs: &HashMap<u32, Value>, xs: Local) -> bool {
    let mut get = false;
    let mut mul = false;
    let mut add = false;
    let mut set = false;
    let mut div = false;
    for_each_def_and_let(body, defs, &mut |v| {
        if let Some((lst, _)) = is_list_get(v) {
            if list_arg_is(lst, xs, defs) {
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
    for_each_def_and_let(body, defs, &mut |v| {
        if let Some((lst, _)) = is_list_get(v) {
            if list_arg_is(lst, xs, defs) {
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
    for_each_def_and_let(body, defs, &mut |v| {
        if let Some((lst, _)) = is_list_get(v) {
            if list_arg_is(lst, xs, defs) {
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
    get && sub && mul && div && !set && body_calls_any(body, &["lumi_f64_sqrt", "sqrtF", "sqrt"])
}

fn fun_has_l2_normalize_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out_slot: &str,
    eps: Local,
) -> bool {
    let mut get = false;
    let mut set = false;
    let mut mul = false;
    let mut uses_eps = false;
    for_each_def_and_let(body, defs, &mut |v| {
        if let Some((lst, _)) = is_list_get(v) {
            if name_of(lst, defs).as_deref() == Some(out_slot) {
                get = true;
            }
        }
        if is_list_set(v).is_some() {
            set = true;
        }
        if matches!(v, Value::Binary { op: BinOp::Mul, .. }) {
            mul = true;
        }
        if mentions_local(v, eps) {
            uses_eps = true;
        }
    });
    get && set && mul && uses_eps && body_calls_any(body, &["lumi_f64_sqrt", "sqrtF", "sqrt"])
}

fn fun_has_softmax_shape(body: &Block, defs: &HashMap<u32, Value>, out_slot: &str) -> bool {
    let mut get = false;
    let mut set = false;
    let mut div = false;
    let mut gt = false;
    for_each_def_and_let(body, defs, &mut |v| {
        if let Some((lst, _)) = is_list_get(v) {
            if name_of(lst, defs).as_deref() == Some(out_slot) {
                get = true;
            }
        }
        if is_list_set(v).is_some() {
            set = true;
        }
        if matches!(v, Value::Binary { op: BinOp::Div, .. }) {
            div = true;
        }
        if matches!(v, Value::Binary { op: BinOp::Gt, .. }) {
            gt = true;
        }
        if matches!(v, Value::If { .. }) {
            gt = true;
        }
    });
    get && set && div && gt && body_calls_any(body, &["lumi_f64_exp", "expF", "exp"])
}

/// `∑ xᵢ²` — get + self-mul + add, no set/div/sqrt.
pub fn match_sum_sq_fun(fun: &CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 1 || !matches!(fun.ret_ty, Type::Float) {
        return None;
    }
    let xs = fun.params[0];
    if !fun_has_sum_sq_shape(&fun.body, defs, xs) {
        return None;
    }
    if body_calls_any(&fun.body, &["lumi_f64_sqrt", "sqrtF", "sqrt"]) {
        return None;
    }
    Some(())
}

/// Arithmetic mean — get + add + div, no set/mul.
pub fn match_mean_fun(fun: &CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 1 || !matches!(fun.ret_ty, Type::Float) {
        return None;
    }
    let xs = fun.params[0];
    if !fun_has_mean_shape(&fun.body, defs, xs) {
        return None;
    }
    Some(())
}

/// `√(∑ xᵢ²)` via scalar `lumi_f64_sqrt` / `sqrt`.
pub fn match_l2_norm_fun(fun: &CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 1 || !matches!(fun.ret_ty, Type::Float) {
        return None;
    }
    let xs = fun.params[0];
    if !fun_has_sum_sq_shape(&fun.body, defs, xs) {
        return None;
    }
    if !body_calls_any(&fun.body, &["lumi_f64_sqrt", "sqrtF", "sqrt"]) {
        return None;
    }
    Some(())
}

/// Population std: variance loop + sqrt (has nontrivial sub).
pub fn match_std_fun(fun: &CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 1 || !matches!(fun.ret_ty, Type::Float) {
        return None;
    }
    let xs = fun.params[0];
    if !fun_has_std_shape(&fun.body, defs, xs) {
        return None;
    }
    Some(())
}

/// In-place L2 normalize with `eps` (set + sqrt + mentions eps).
pub fn match_l2_normalize_fun(fun: &CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 2 {
        return None;
    }
    let (xs, eps) = (fun.params[0], fun.params[1]);
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, xs)?;
    if !fun_has_l2_normalize_shape(body, defs, &out_slot, eps) {
        return None;
    }
    Some(())
}

/// Softmax: max pass + exp + normalize (set + exp call + Gt).
pub fn match_softmax_fun(fun: &CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 1 {
        return None;
    }
    let xs = fun.params[0];
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, xs)?;
    if !fun_has_softmax_shape(body, defs, &out_slot) {
        return None;
    }
    Some(())
}

/// `zeros(n)` via `listOf(0.0)` + `append(0.0)` loop (or empty + append from 0).
pub fn match_zeros_fun(fun: &CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 1 {
        return None;
    }
    let n = fun.params[0];
    let body = &fun.body;
    let mut seed = false;
    let mut append0 = false;
    let mut bound_n = false;
    for_each_def_and_let(body, defs, &mut |v| {
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
        if let Value::Loop { header, .. } = v {
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

pub fn mentions_local(v: &Value, target: Local) -> bool {
    match v {
        Value::Local(l) => *l == target,
        Value::Binary { left, right, .. } => *left == target || *right == target,
        Value::Builtin { args, .. } => args.contains(&target),
        _ => false,
    }
}

/// Opt rewrite symbol for a whole-function dense kernel (order-sensitive).
pub fn dense_f64_rt_symbol(fun: &CoreFun, defs: &HashMap<u32, Value>) -> Option<&'static str> {
    if match_gemv_fun(fun, defs).is_some() {
        Some("lumi_f64_gemv")
    } else if match_gemv_t_fun(fun, defs).is_some() {
        Some("lumi_f64_gemv_t")
    } else if match_addmm_fun(fun, defs).is_some() {
        Some("lumi_f64_addmm")
    } else if match_axpy_fun(fun, defs).is_some() {
        Some("lumi_f64_axpy")
    } else if match_sub_fun(fun, defs).is_some() {
        Some("lumi_f64_sub")
    } else if match_add_fun(fun, defs).is_some() {
        Some("lumi_f64_add")
    } else if match_mul_fun(fun, defs).is_some() {
        Some("lumi_f64_mul")
    } else if match_clamp_fun(fun, defs).is_some() {
        Some("lumi_f64_clamp")
    } else if match_scale_fun(fun, defs).is_some() {
        Some("lumi_f64_scale")
    } else if match_fill_fun(fun, defs).is_some() {
        Some("lumi_f64_fill")
    } else if match_copy_fun(fun, defs).is_some() {
        Some("lumi_f64_copy")
    } else if match_zeros_fun(fun, defs).is_some() {
        Some("lumi_list_f64_zeros")
    } else if match_l2_normalize_fun(fun, defs).is_some() {
        Some("lumi_f64_l2_normalize")
    } else if match_softmax_fun(fun, defs).is_some() {
        Some("lumi_f64_softmax")
    } else if match_l2_norm_fun(fun, defs).is_some() {
        Some("lumi_f64_l2_norm")
    } else if match_std_fun(fun, defs).is_some() {
        Some("lumi_f64_std")
    } else if match_sum_sq_fun(fun, defs).is_some() {
        Some("lumi_f64_sum_sq")
    } else if match_mean_fun(fun, defs).is_some() {
        Some("lumi_f64_mean")
    } else {
        None
    }
}

/// Visit leaf `defs` then every `Let` value under `body` (same predicate over both).
pub fn for_each_def_and_let(body: &Block, defs: &HashMap<u32, Value>, f: &mut dyn FnMut(&Value)) {
    for v in defs.values() {
        f(v);
    }
    crate::visit::for_each_let(body, f);
}
