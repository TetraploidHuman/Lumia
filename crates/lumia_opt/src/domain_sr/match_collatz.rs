//! Whole-function Collatz total / strided matchers.

use super::externs::RtArg;
use lumia_core::CoreBinOp as BinOp;
use lumia_core::{
    body_assigns_name_div_const, body_assigns_name_mul_const_plus_const, const_of,
    first_direct_loop, header_gt_eq, header_le_bound, header_le_const, is_add_name_plus_name,
    is_name_add_const, is_name_rem_eq_const, is_unit_inc, name_of, same_local, Block, CoreFun,
    Local, Op, Value,
};
use lumia_ty::Type;
use rustc_hash::FxHashMap as HashMap;

/// `collatzTotal(limit)` with `n=1`, `total=0`, outer `n<=limit`, nested steps.
///
/// Also matches fully const-specialized `$c_` clones (`params` empty).
pub(super) fn match_collatz_total_fun(
    fun: &CoreFun,
    defs: &HashMap<u32, Value>,
) -> Option<Vec<RtArg>> {
    if fun.params.len() > 1 || fun.ret_ty != Type::Int {
        return None;
    }
    if !slot_init_const(&fun.body, "n", 1, defs) || !slot_init_const(&fun.body, "total", 0, defs) {
        return None;
    }
    let (header, body, latch) = first_direct_loop(&fun.body)?;
    if !latch.ops.is_empty() {
        return None;
    }
    let p0 = fun.params.first().copied().unwrap_or(Local(u32::MAX));
    let (n, limit) = outer_le_limit(header, defs, p0, fun.param_names.first())?;
    if n != "n" {
        return None;
    }
    if !body_has_collatz_steps(body, &n, defs) {
        return None;
    }
    if !body_adds_steps_to_total(body, &n, defs) {
        return None;
    }
    if !body_unit_incs_slot(body, &n, defs) {
        return None;
    }
    if !result_is_slot(&fun.body, "total", defs) {
        return None;
    }
    Some(match limit {
        Some(c) => vec![RtArg::Const(c)],
        None => vec![RtArg::Param(0)],
    })
}

/// `collatzStrided(start, limit, stride)` with `n=start`, `total=0`, `n+=stride`.
///
/// Also matches fully const-specialized `$c_` clones (`params` empty).
pub(super) fn match_collatz_strided_fun(
    fun: &CoreFun,
    defs: &HashMap<u32, Value>,
) -> Option<Vec<RtArg>> {
    if fun.ret_ty != Type::Int {
        return None;
    }
    let specialized = fun.params.is_empty();
    if !specialized && fun.params.len() != 3 {
        return None;
    }
    if !slot_init_const(&fun.body, "total", 0, defs) {
        return None;
    }
    let start_arg = if specialized {
        let c = slot_init_const_value(&fun.body, "n", defs)?;
        RtArg::Const(c)
    } else {
        if !slot_init_from_param(&fun.body, "n", fun.params[0], defs) {
            return None;
        }
        RtArg::Param(0)
    };
    let (header, body, latch) = first_direct_loop(&fun.body)?;
    if !latch.ops.is_empty() {
        return None;
    }
    let limit_param = fun.params.get(1).copied().unwrap_or(Local(u32::MAX));
    let (n, limit) = outer_le_limit(header, defs, limit_param, fun.param_names.get(1))?;
    if n != "n" {
        return None;
    }
    if !body_has_collatz_steps(body, &n, defs) {
        return None;
    }
    if !body_adds_steps_to_total(body, &n, defs) {
        return None;
    }
    let stride_arg = match body_stride_add(body, &n, defs) {
        Some(k) if k >= 2 => RtArg::Const(k),
        Some(_) => return None,
        None => {
            if specialized {
                return None;
            }
            if !body_adds_param_stride(body, &n, fun.params[2], defs) {
                return None;
            }
            RtArg::Param(2)
        }
    };
    if !result_is_slot(&fun.body, "total", defs) {
        return None;
    }
    let limit_arg = match limit {
        Some(c) => RtArg::Const(c),
        None => RtArg::Param(1),
    };
    Some(vec![start_arg, limit_arg, stride_arg])
}

fn outer_le_limit(
    header: &Block,
    defs: &HashMap<u32, Value>,
    limit_param: Local,
    param_name: Option<&String>,
) -> Option<(String, Option<i64>)> {
    if let Some((n, c)) = header_le_const(header, defs) {
        if c >= 1 {
            return Some((n, Some(c)));
        }
    }
    let (n, bound) = header_le_bound(header, defs)?;
    if same_local(bound, limit_param, defs) {
        return Some((n, None));
    }
    if let Some(Value::Name(nm)) = defs.get(&bound.0) {
        if param_name == Some(nm) {
            return Some((n, None));
        }
    }
    None
}

fn slot_init_const(body: &Block, slot: &str, expect: i64, defs: &HashMap<u32, Value>) -> bool {
    slot_init_const_value(body, slot, defs) == Some(expect)
}

fn slot_init_const_value(body: &Block, slot: &str, defs: &HashMap<u32, Value>) -> Option<i64> {
    for op in &body.ops {
        if matches!(
            op,
            Op::Let {
                value: Value::Loop { .. },
                ..
            }
        ) {
            break;
        }
        if let Op::Assign {
            name,
            value: Local(v),
        } = op
        {
            if name == slot {
                return const_of(Local(*v), defs);
            }
        }
    }
    None
}

fn slot_init_from_param(
    body: &Block,
    slot: &str,
    param: Local,
    defs: &HashMap<u32, Value>,
) -> bool {
    for op in &body.ops {
        if matches!(
            op,
            Op::Let {
                value: Value::Loop { .. },
                ..
            }
        ) {
            break;
        }
        if let Op::Assign {
            name,
            value: Local(v),
        } = op
        {
            if name == slot && same_local(Local(*v), param, defs) {
                return true;
            }
        }
    }
    false
}

fn result_is_slot(body: &Block, slot: &str, defs: &HashMap<u32, Value>) -> bool {
    let Some(r) = body.result else {
        return false;
    };
    name_of(r, defs).as_deref() == Some(slot)
}

fn body_unit_incs_slot(body: &Block, slot: &str, defs: &HashMap<u32, Value>) -> bool {
    for op in &body.ops {
        if let Op::Assign {
            name,
            value: Local(v),
        } = op
        {
            if name == slot && is_unit_inc(*v, slot, defs) {
                return true;
            }
        }
    }
    false
}

fn body_stride_add(body: &Block, n: &str, defs: &HashMap<u32, Value>) -> Option<i64> {
    for op in &body.ops {
        if let Op::Assign {
            name,
            value: Local(v),
        } = op
        {
            if name == n {
                if let Some(k) = is_name_add_const(*v, n, defs) {
                    return Some(k);
                }
            }
        }
    }
    None
}

fn body_adds_param_stride(
    body: &Block,
    n: &str,
    stride_param: Local,
    defs: &HashMap<u32, Value>,
) -> bool {
    for op in &body.ops {
        if let Op::Assign {
            name,
            value: Local(v),
        } = op
        {
            if name != n {
                continue;
            }
            let Some(Value::Binary {
                op: BinOp::Add,
                left,
                right,
                ..
            }) = defs.get(v)
            else {
                continue;
            };
            let ln = name_of(*left, defs);
            let rn = name_of(*right, defs);
            let l_self = ln.as_deref() == Some(n);
            let r_self = rn.as_deref() == Some(n);
            let l_stride = same_local(*left, stride_param, defs);
            let r_stride = same_local(*right, stride_param, defs);
            if (l_self && r_stride) || (r_self && l_stride) {
                return true;
            }
        }
    }
    false
}

fn body_adds_steps_to_total(body: &Block, n: &str, defs: &HashMap<u32, Value>) -> bool {
    for op in &body.ops {
        if let Op::Assign {
            name,
            value: Local(v),
        } = op
        {
            if name == n || name == "steps" {
                continue;
            }
            if is_add_total_plus_rhs(*v, name, defs)
                || is_add_name_plus_name(*v, name, "steps", defs)
            {
                return true;
            }
        }
    }
    false
}

/// `total = total + <rhs>` where rhs may be a Call result (no Name).
fn is_add_total_plus_rhs(v: u32, total: &str, defs: &HashMap<u32, Value>) -> bool {
    let Some(Value::Binary {
        op: BinOp::Add,
        left,
        right,
        ..
    }) = defs.get(&v)
    else {
        return false;
    };
    let ln = name_of(*left, defs);
    let rn = name_of(*right, defs);
    (ln.as_deref() == Some(total) && rn.as_deref() != Some(total))
        || (rn.as_deref() == Some(total) && ln.as_deref() != Some(total))
}

fn body_has_collatz_steps(body: &Block, n: &str, defs: &HashMap<u32, Value>) -> bool {
    for op in &body.ops {
        if let Op::Let {
            value:
                Value::Loop {
                    header,
                    body: ib,
                    latch,
                },
            ..
        } = op
        {
            if match_collatz_loop(header, ib, latch, defs).is_some() {
                return true;
            }
        }
        if let Op::Let {
            value: Value::Call { fun, args },
            ..
        } = op
        {
            if is_collatz_steps_fun(fun.as_str()) && args.len() == 1 {
                if name_of(args[0], defs).as_deref() == Some(n) {
                    return true;
                }
            }
        }
    }
    false
}

fn is_collatz_steps_fun(name: &str) -> bool {
    name == "collatzSteps" || name.starts_with("collatzSteps$")
}

fn match_collatz_loop(
    header: &Block,
    body: &Block,
    latch: &Block,
    defs: &HashMap<u32, Value>,
) -> Option<()> {
    if !latch.ops.is_empty() {
        return None;
    }
    let x = header_gt_eq(header, 1, defs)?;
    let (then_div, else_triple, _steps) = body_collatz_parts(body, &x, defs)?;
    if then_div && else_triple {
        Some(())
    } else {
        None
    }
}

fn body_collatz_parts(
    body: &Block,
    x: &str,
    defs: &HashMap<u32, Value>,
) -> Option<(bool, bool, String)> {
    let mut then_div = false;
    let mut else_triple = false;
    let mut steps: Option<String> = None;
    for op in &body.ops {
        match op {
            Op::Let {
                value:
                    Value::If {
                        cond,
                        then_block,
                        else_block,
                    },
                ..
            } => {
                if !is_name_rem_eq_const(*cond, x, 2, defs) {
                    return None;
                }
                then_div = body_assigns_name_div_const(then_block, x, 2, defs);
                else_triple = body_assigns_name_mul_const_plus_const(else_block, x, 3, 1, defs);
            }
            Op::Assign {
                name,
                value: Local(v),
            } => {
                if is_unit_inc(*v, name, defs) {
                    steps = Some(name.clone());
                }
            }
            _ => {}
        }
    }
    Some((then_div, else_triple, steps?))
}
