//! Whole-function Collatz total / strided / steps matchers.

use super::externs::RtArg;
use lumia_core::{
    body_assigns_acc_plus_steps, body_assigns_name_add_const, body_assigns_name_add_local,
    body_assigns_name_div_const, body_assigns_name_mul_const_plus_const, body_assigns_unit_inc,
    first_direct_loop, for_each_let_in_block, for_each_top_level_op_in_block, header_gt_eq,
    is_name_rem_eq_const, is_unit_inc, name_of, outer_le_param_or_const, result_is_slot,
    slot_init_const, slot_init_const_value, slot_init_from_param, Block, CoreFun, Local, Op, Value,
};
use lumia_ty::Type;
use rustc_hash::FxHashMap as HashMap;

/// `collatzSteps(n)` with `x=n`, `steps=0`, inner `for x > 1` Collatz loop.
///
/// Also matches fully const-specialized `$c_` clones (`params` empty).
pub(super) fn match_collatz_steps_fun(
    fun: &CoreFun,
    defs: &HashMap<u32, Value>,
) -> Option<Vec<RtArg>> {
    if !is_collatz_steps_fun(&fun.name) || fun.ret_ty != Type::Int {
        return None;
    }
    let specialized = fun.params.is_empty();
    if !specialized && fun.params.len() != 1 {
        return None;
    }
    if !slot_init_const(&fun.body, "steps", 0, defs) {
        return None;
    }
    let n_arg = if specialized {
        RtArg::Const(slot_init_const_value(&fun.body, "x", defs)?)
    } else {
        if !slot_init_from_param(&fun.body, "x", fun.params[0], defs) {
            return None;
        }
        RtArg::Param(0)
    };
    let (header, body, latch) = first_direct_loop(&fun.body)?;
    if !latch.ops.is_empty() {
        return None;
    }
    let x = header_gt_eq(header, 1, defs)?;
    if x != "x" {
        return None;
    }
    let (then_div, else_triple, steps) = body_collatz_parts(body, &x, defs)?;
    if !then_div || !else_triple || steps != "steps" {
        return None;
    }
    if !result_is_slot(&fun.body, "steps", defs) {
        return None;
    }
    Some(vec![n_arg])
}

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
    let (n, limit) = outer_le_param_or_const(
        header,
        defs,
        p0,
        fun.param_names.first().map(|s| s.as_str()),
        1,
    )?;
    if n != "n" {
        return None;
    }
    if !body_has_collatz_steps(body, &n, defs) {
        return None;
    }
    if !body_assigns_acc_plus_steps(body, &["n", "steps"], "steps", defs) {
        return None;
    }
    if !body_assigns_unit_inc(body, &n, defs) {
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
    let (n, limit) = outer_le_param_or_const(
        header,
        defs,
        limit_param,
        fun.param_names.get(1).map(|s| s.as_str()),
        1,
    )?;
    if n != "n" {
        return None;
    }
    if !body_has_collatz_steps(body, &n, defs) {
        return None;
    }
    if !body_assigns_acc_plus_steps(body, &["n", "steps"], "steps", defs) {
        return None;
    }
    let stride_arg = match body_assigns_name_add_const(body, &n, defs) {
        Some(k) if k >= 2 => RtArg::Const(k),
        Some(_) => return None,
        None => {
            if specialized {
                return None;
            }
            if !body_assigns_name_add_local(body, &n, fun.params[2], defs) {
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

fn body_has_collatz_steps(body: &Block, n: &str, defs: &HashMap<u32, Value>) -> bool {
    let mut found = false;
    for_each_let_in_block(body, &mut |_local, value, _pure| {
        if found {
            return;
        }
        if let Value::Loop {
            header,
            body: ib,
            latch,
        } = value
        {
            if match_collatz_loop(header, ib, latch, defs).is_some() {
                found = true;
                return;
            }
        }
        if let Value::Call { fun, args } = value {
            if is_collatz_steps_fun(fun.as_str()) && args.len() == 1 {
                if name_of(args[0], defs).as_deref() == Some(n) {
                    found = true;
                }
            }
        }
    });
    found
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
    let mut invalid = false;
    for_each_top_level_op_in_block(body, &mut |op| {
        if invalid {
            return;
        }
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
                    invalid = true;
                    return;
                }
                then_div = body_assigns_name_div_const(then_block, x, 2, defs);
                else_triple = body_assigns_name_mul_const_plus_const(else_block, x, 3, 1, defs);
            }
            Op::Assign {
                name,
                value: Local(v),
            } => {
                if is_unit_inc(*v, name, defs) {
                    steps = Some(name.to_string());
                }
            }
            _ => {}
        }
    });
    if invalid {
        return None;
    }
    Some((then_div, else_triple, steps?))
}
