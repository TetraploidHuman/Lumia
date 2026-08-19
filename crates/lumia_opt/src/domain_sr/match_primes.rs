//! Whole-function `countPrimes(limit)` matcher.

use super::externs::RtArg;
use lumia_core::{
    body_assigns_unit_inc, first_direct_loop, for_each_let_in_block,
    for_each_top_level_op_in_block, header_name_sq_cmp, is_unit_inc, local_is_zero_or_false,
    name_of, outer_le_param_or_const, rem_eq_zero_names, rem_eq_zero_operands, result_is_slot,
    same_local, slot_init_const, slot_init_zero_name, Block, CoreFun, Local, Op, Value,
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
    let count_slot = slot_init_zero_name(&fun.body, defs)?;
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
        2,
    )?;
    if n != "n" {
        return None;
    }
    if !body_has_trial_or_is_prime(body, &n, defs) {
        return None;
    }
    if !body_assigns_unit_inc(body, &n, defs) {
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

fn body_has_count_inc(body: &Block, n: &str, count: &str, defs: &HashMap<u32, Value>) -> bool {
    let mut found = false;
    for_each_let_in_block(body, &mut |_local, value, _pure| {
        if found {
            return;
        }
        match value {
            Value::If {
                then_block,
                else_block,
                ..
            } => {
                if body_assigns_unit_inc(then_block, count, defs)
                    || body_assigns_unit_inc(else_block, count, defs)
                    || body_has_count_inc(then_block, n, count, defs)
                    || body_has_count_inc(else_block, n, count, defs)
                {
                    found = true;
                }
            }
            Value::Loop { body: ib, .. } => {
                if body_has_count_inc(ib, n, count, defs) {
                    found = true;
                }
            }
            _ => {}
        }
    });
    if found {
        return true;
    }
    lumia_core::for_each_assign_in_block(body, &mut |name, v| {
        if !found && name == count && is_unit_inc(v.0, count, defs) {
            found = true;
        }
    });
    found
}

fn body_has_trial_or_is_prime(body: &Block, n: &str, defs: &HashMap<u32, Value>) -> bool {
    let mut found = false;
    for_each_let_in_block(body, &mut |_local, value, _pure| {
        if found {
            return;
        }
        match value {
            Value::Loop {
                header,
                body: ib,
                latch,
            } => {
                if let Some(p) = match_trial_div_loop(header, ib, latch, defs) {
                    if p.n == n {
                        found = true;
                        return;
                    }
                }
                if body_has_trial_or_is_prime(ib, n, defs) {
                    found = true;
                }
            }
            Value::Call { fun, args } => {
                if is_is_prime_fun(fun.as_str()) && args.len() == 1 {
                    if name_of(args[0], defs).as_deref() == Some(n) {
                        found = true;
                    }
                }
            }
            Value::If {
                then_block,
                else_block,
                ..
            } => {
                if body_has_trial_or_is_prime(then_block, n, defs)
                    || body_has_trial_or_is_prime(else_block, n, defs)
                {
                    found = true;
                }
            }
            _ => {}
        }
    });
    found
}

fn is_is_prime_fun(name: &str) -> bool {
    name == "isPrime" || name.starts_with("isPrime$")
}

pub(super) struct TrialDiv {
    pub d: lumia_syntax::Sym,
    pub n: lumia_syntax::Sym,
}

pub(super) fn match_trial_div_loop(
    header: &Block,
    body: &Block,
    latch: &Block,
    defs: &HashMap<u32, Value>,
) -> Option<TrialDiv> {
    if !latch.ops.is_empty() {
        return None;
    }
    let (d, bound, strict) = header_name_sq_cmp(header, defs)?;
    if strict {
        return None;
    }
    // Bound is `Name(n)` after inline, or the `isPrime(n)` param local in Debug.
    let n = name_of(bound, defs).unwrap_or_else(|| lumia_syntax::Sym::from(""));
    let _ok = body_trial_parts(body, &d, bound, &n, defs)?;
    Some(TrialDiv { d, n })
}

fn rem_n_mod_d_zero(
    cond: Local,
    n_local: Local,
    n_name: &str,
    d: &str,
    defs: &HashMap<u32, Value>,
) -> bool {
    if !n_name.is_empty() && rem_eq_zero_names(cond, n_name, d, defs) {
        return true;
    }
    let Some((a, b)) = rem_eq_zero_operands(cond, defs) else {
        return false;
    };
    let is_n = |l: Local| {
        same_local(l, n_local, defs)
            || (!n_name.is_empty() && name_of(l, defs).as_deref() == Some(n_name))
    };
    let is_d = |l: Local| name_of(l, defs).as_deref() == Some(d);
    (is_n(a) && is_d(b)) || (is_n(b) && is_d(a))
}

fn body_trial_parts(
    body: &Block,
    d: &str,
    n_local: Local,
    n_name: &str,
    defs: &HashMap<u32, Value>,
) -> Option<String> {
    let mut ok_name: Option<String> = None;
    let mut saw_break = false;
    let mut saw_step = false;
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
                if !rem_n_mod_d_zero(*cond, n_local, n_name, d, defs) {
                    invalid = true;
                    return;
                }
                let mut then_defs = defs.clone();
                for_each_let_in_block(then_block, &mut |local, value, _pure| {
                    then_defs.insert(local.0, value.clone());
                });
                for_each_top_level_op_in_block(then_block, &mut |top| match top {
                    Op::Assign {
                        name,
                        value: Local(v),
                    } if local_is_zero_or_false(Local(*v), &then_defs) => {
                        ok_name = Some(name.to_string());
                    }
                    Op::Break => saw_break = true,
                    _ => {}
                });
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
    });
    if invalid {
        return None;
    }
    if saw_break && saw_step {
        ok_name
    } else {
        None
    }
}
