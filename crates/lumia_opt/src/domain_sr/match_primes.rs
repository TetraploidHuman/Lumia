//! Whole-function `countPrimes(limit)` matcher.

use super::externs::RtArg;
use lumia_core::{
    body_assigns_unit_inc, const_of, first_direct_loop, header_le_bound, header_le_const,
    header_name_sq_le_name, is_unit_inc, local_is_zero_or_false, name_of, rem_eq_zero_names,
    same_local, Block, CoreFun, Local, Op, Value,
};
use lumia_ty::Type;
use rustc_hash::FxHashMap as HashMap;

/// `countPrimes(limit)` with `n=2`, `c=0`, outer `n<=limit`, trial / `isPrime`.
///
/// Also matches fully const-specialized `$c_` clones (`params` empty).
pub(super) fn match_count_primes_fun(
    fun: &CoreFun,
    defs: &HashMap<u32, Value>,
) -> Option<Vec<RtArg>> {
    if fun.params.len() > 1 || fun.ret_ty != Type::Int {
        return None;
    }
    if !slot_init_const(&fun.body, "n", 2, defs) {
        return None;
    }
    let count_slot = count_slot_init_zero(&fun.body, defs)?;
    let (header, body, latch) = first_direct_loop(&fun.body)?;
    if !latch.ops.is_empty() {
        return None;
    }
    let p0 = fun.params.first().copied().unwrap_or(Local(u32::MAX));
    let (n, limit) = outer_le_limit(header, defs, p0, fun.param_names.first())?;
    if n != "n" {
        return None;
    }
    if !body_has_trial_or_is_prime(body, &n, defs) {
        return None;
    }
    if !body_unit_incs_slot(body, &n, defs) {
        return None;
    }
    if !body_has_count_inc(body, &n, &count_slot, defs) {
        return None;
    }
    if !result_is_slot(&fun.body, &count_slot, defs) {
        return None;
    }
    Some(match limit {
        Some(c) => vec![RtArg::Const(c)],
        None => vec![RtArg::Param(0)],
    })
}

/// `(iv_name, Some(const_limit))` or `(iv_name, None)` when bound is the param.
fn outer_le_limit(
    header: &Block,
    defs: &HashMap<u32, Value>,
    limit_param: Local,
    param_name: Option<&String>,
) -> Option<(String, Option<i64>)> {
    if let Some((n, c)) = header_le_const(header, defs) {
        if c >= 2 {
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
            if name == slot && const_of(Local(*v), defs) == Some(expect) {
                return true;
            }
        }
    }
    false
}

fn count_slot_init_zero(body: &Block, defs: &HashMap<u32, Value>) -> Option<String> {
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
            if const_of(Local(*v), defs) == Some(0) {
                return Some(name.clone());
            }
        }
    }
    None
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

fn body_has_count_inc(body: &Block, n: &str, count: &str, defs: &HashMap<u32, Value>) -> bool {
    for op in &body.ops {
        if let Op::Let {
            value:
                Value::If {
                    then_block,
                    else_block,
                    ..
                },
            ..
        } = op
        {
            if body_assigns_unit_inc(then_block, count, defs)
                || body_assigns_unit_inc(else_block, count, defs)
            {
                return true;
            }
            if body_has_count_inc(then_block, n, count, defs)
                || body_has_count_inc(else_block, n, count, defs)
            {
                return true;
            }
        }
        if let Op::Let {
            value: Value::Loop { body: ib, .. },
            ..
        } = op
        {
            if body_has_count_inc(ib, n, count, defs) {
                return true;
            }
        }
        if let Op::Assign {
            name,
            value: Local(v),
        } = op
        {
            if name == count && is_unit_inc(*v, count, defs) {
                return true;
            }
        }
    }
    false
}

fn body_has_trial_or_is_prime(body: &Block, n: &str, defs: &HashMap<u32, Value>) -> bool {
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
            if let Some(p) = match_trial_div_loop(header, ib, latch, defs) {
                if p.n == n {
                    return true;
                }
            }
            if body_has_trial_or_is_prime(ib, n, defs) {
                return true;
            }
        }
        if let Op::Let {
            value: Value::Call { fun, args },
            ..
        } = op
        {
            if is_is_prime_fun(fun.as_str()) && args.len() == 1 {
                if name_of(args[0], defs).as_deref() == Some(n) {
                    return true;
                }
            }
        }
        if let Op::Let {
            value:
                Value::If {
                    then_block,
                    else_block,
                    ..
                },
            ..
        } = op
        {
            if body_has_trial_or_is_prime(then_block, n, defs)
                || body_has_trial_or_is_prime(else_block, n, defs)
            {
                return true;
            }
        }
    }
    false
}

fn is_is_prime_fun(name: &str) -> bool {
    name == "isPrime" || name.starts_with("isPrime$")
}

struct TrialDiv {
    n: String,
}

fn match_trial_div_loop(
    header: &Block,
    body: &Block,
    latch: &Block,
    defs: &HashMap<u32, Value>,
) -> Option<TrialDiv> {
    if !latch.ops.is_empty() {
        return None;
    }
    let (d, n) = header_name_sq_le_name(header, defs)?;
    let _ok = body_trial_parts(body, &d, &n, defs)?;
    Some(TrialDiv { n })
}

fn body_trial_parts(body: &Block, d: &str, n: &str, defs: &HashMap<u32, Value>) -> Option<String> {
    let mut ok_name: Option<String> = None;
    let mut saw_break = false;
    let mut saw_step = false;
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
                if !rem_eq_zero_names(*cond, n, d, defs) {
                    return None;
                }
                let mut then_defs = defs.clone();
                for top in &then_block.ops {
                    if let Op::Let { local, value, .. } = top {
                        then_defs.insert(local.0, value.clone());
                    }
                }
                for top in &then_block.ops {
                    match top {
                        Op::Assign {
                            name,
                            value: Local(v),
                        } if local_is_zero_or_false(Local(*v), &then_defs) => {
                            ok_name = Some(name.clone());
                        }
                        Op::Break => saw_break = true,
                        _ => {}
                    }
                }
                if body_assigns_unit_inc(else_block, d, defs) {
                    saw_step = true;
                }
            }
            Op::Assign {
                name,
                value: Local(v),
            } if name == d && is_unit_inc(*v, d, defs) => {
                saw_step = true;
            }
            _ => {}
        }
    }
    if saw_break && saw_step {
        ok_name
    } else {
        None
    }
}
