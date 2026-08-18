//! Shared Core IR pattern primitives for loop / nest shape-rewrite (`*_sr`).
//!
//! Used by codegen domain SRs and `lumia_opt::dense_f64_sr` so name/const/IV
//! peeps cannot drift (Todo: 领域 SR / dense_f64 缺共用匹配原语).

use crate::{Block, CoreBinOp as BinOp, Local, Op, Value};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

pub fn name_of(l: Local, defs: &HashMap<u32, Value>) -> Option<String> {
    match defs.get(&l.0)? {
        Value::Name(n) => Some(n.clone()),
        _ => None,
    }
}

pub fn const_of(l: Local, defs: &HashMap<u32, Value>) -> Option<i64> {
    match defs.get(&l.0)? {
        Value::Int(n) => Some(*n),
        _ => None,
    }
}

/// `dest = name + 1` or `dest = 1 + name`.
pub fn is_unit_inc(dest: u32, name: &str, defs: &HashMap<u32, Value>) -> bool {
    matches!(defs.get(&dest), Some(Value::Binary { op: BinOp::Add, .. }))
        && is_unit_step(dest, name, defs)
}

/// `dest = name ± 1` (Add `+1` either side, or Sub `name - 1`).
pub fn is_unit_step(dest: u32, name: &str, defs: &HashMap<u32, Value>) -> bool {
    let Some(Value::Binary {
        op, left, right, ..
    }) = defs.get(&dest)
    else {
        return false;
    };
    let l_iv = name_of(*left, defs).as_deref() == Some(name);
    let r_iv = name_of(*right, defs).as_deref() == Some(name);
    let l_c = const_of(*left, defs);
    let r_c = const_of(*right, defs);
    match op {
        BinOp::Add => (l_iv && r_c == Some(1)) || (r_iv && l_c == Some(1)),
        BinOp::Sub => l_iv && r_c == Some(1),
        _ => false,
    }
}

/// Cond is `Name(name)` or `Name(name) != 0` / `0 != Name(name)`.
pub fn is_name_ne_zero(cond: Local, name: &str, defs: &HashMap<u32, Value>) -> bool {
    if name_of(cond, defs).as_deref() == Some(name) {
        return true;
    }
    name_ne_zero(cond, defs).as_deref() == Some(name)
}

/// Binary `Name(y) != 0` / `0 != Name(y)` → the slot name (Euclid header).
///
/// Does **not** accept a bare `Name` cond (unlike [`is_name_ne_zero`]).
pub fn name_ne_zero(cond: Local, defs: &HashMap<u32, Value>) -> Option<String> {
    let Some(Value::Binary {
        op: BinOp::Ne,
        left,
        right,
        ..
    }) = defs.get(&cond.0)
    else {
        return None;
    };
    match (name_of(*left, defs), const_of(*right, defs)) {
        (Some(n), Some(0)) => Some(n),
        _ => match (name_of(*right, defs), const_of(*left, defs)) {
            (Some(n), Some(0)) => Some(n),
            _ => None,
        },
    }
}

/// Header result is `Name(iv) < k` (or `k > Name(iv)`).
pub fn header_lt_const(header: &Block, defs: &HashMap<u32, Value>) -> Option<(String, i64)> {
    let res = header.result?;
    let Value::Binary {
        op, left, right, ..
    } = defs.get(&res.0)?
    else {
        return None;
    };
    match op {
        BinOp::Lt => {
            let iv = name_of(*left, defs)?;
            let k = const_of(*right, defs)?;
            Some((iv, k))
        }
        BinOp::Gt => {
            let iv = name_of(*right, defs)?;
            let k = const_of(*left, defs)?;
            Some((iv, k))
        }
        _ => None,
    }
}

/// Header result is `Name(iv) > k` (or `k < Name(iv)`).
pub fn header_gt_const(header: &Block, defs: &HashMap<u32, Value>) -> Option<(String, i64)> {
    let res = header.result?;
    let Value::Binary {
        op, left, right, ..
    } = defs.get(&res.0)?
    else {
        return None;
    };
    match op {
        BinOp::Gt => {
            let iv = name_of(*left, defs)?;
            let k = const_of(*right, defs)?;
            Some((iv, k))
        }
        BinOp::Lt => {
            let iv = name_of(*right, defs)?;
            let k = const_of(*left, defs)?;
            Some((iv, k))
        }
        _ => None,
    }
}

/// Header result is `Name(iv) >= k` (IV on the left only).
pub fn header_ge_const(header: &Block, defs: &HashMap<u32, Value>) -> Option<(String, i64)> {
    let res = header.result?;
    let Value::Binary {
        op: BinOp::Ge,
        left,
        right,
        ..
    } = defs.get(&res.0)?
    else {
        return None;
    };
    let iv = name_of(*left, defs)?;
    let k = const_of(*right, defs)?;
    Some((iv, k))
}

/// Header result is `Name(iv) > k` for a specific const `k` (Collatz `x > 1`).
pub fn header_gt_eq(header: &Block, k: i64, defs: &HashMap<u32, Value>) -> Option<String> {
    let (iv, got) = header_gt_const(header, defs)?;
    (got == k).then_some(iv)
}

/// Header result is `Name(iv) <= k` (or `k >= Name(iv)`).
pub fn header_le_const(header: &Block, defs: &HashMap<u32, Value>) -> Option<(String, i64)> {
    let res = header.result?;
    let Value::Binary {
        op, left, right, ..
    } = defs.get(&res.0)?
    else {
        return None;
    };
    match op {
        BinOp::Le => {
            let name = name_of(*left, defs)?;
            let n = const_of(*right, defs)?;
            Some((name, n))
        }
        BinOp::Ge => {
            let name = name_of(*right, defs)?;
            let n = const_of(*left, defs)?;
            Some((name, n))
        }
        _ => None,
    }
}

/// Header result is `Name(iv) < bound` where `bound` is a Local (not necessarily const).
pub fn header_lt_bound(header: &Block, defs: &HashMap<u32, Value>) -> Option<(String, Local)> {
    let res = header.result?;
    let Value::Binary {
        op: BinOp::Lt,
        left,
        right,
        ..
    } = defs.get(&res.0)?
    else {
        return None;
    };
    let iv = name_of(*left, defs)?;
    Some((iv, *right))
}

/// Header result is `Name(iv) <= bound` (or `bound >= Name(iv)`) where `bound` is a Local.
pub fn header_le_bound(header: &Block, defs: &HashMap<u32, Value>) -> Option<(String, Local)> {
    let res = header.result?;
    let Value::Binary {
        op, left, right, ..
    } = defs.get(&res.0)?
    else {
        return None;
    };
    match op {
        BinOp::Le => {
            let iv = name_of(*left, defs)?;
            Some((iv, *right))
        }
        BinOp::Ge => {
            let iv = name_of(*right, defs)?;
            Some((iv, *left))
        }
        _ => None,
    }
}

/// Resolve `Local` / identity through `Value::Local` leaf defs.
pub fn same_local(got: Local, want: Local, defs: &HashMap<u32, Value>) -> bool {
    if got == want {
        return true;
    }
    match defs.get(&got.0) {
        Some(Value::Local(l)) => same_local(*l, want, defs),
        Some(Value::Name(_)) => false, // slot load ≠ param unless assigned from it
        _ => false,
    }
}

/// `ListGet(list, idx)` peep.
pub fn is_list_get(v: &Value) -> Option<(Local, Local)> {
    match v {
        Value::Builtin {
            name: lumia_hir::Builtin::ListGet,
            args,
            ..
        } if args.len() == 2 => Some((args[0], args[1])),
        _ => None,
    }
}

/// Dense nest mutation peep: `MapSet` used as list element update.
pub fn is_list_set(v: &Value) -> Option<(Local, Local, Local)> {
    match v {
        Value::Builtin {
            name: lumia_hir::Builtin::MapSet,
            args,
            ..
        } if args.len() == 3 => Some((args[0], args[1], args[2])),
        _ => None,
    }
}

pub fn body_assigns_const(
    body: &Block,
    slot: &str,
    expect: i64,
    defs: &HashMap<u32, Value>,
) -> bool {
    for op in &body.ops {
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

/// Whether `block` assigns `slot = 0` or `slot = false` (trial_div prime-flag clear).
pub fn body_assigns_zero_or_false(body: &Block, slot: &str, defs: &HashMap<u32, Value>) -> bool {
    for op in &body.ops {
        if let Op::Assign {
            name,
            value: Local(v),
        } = op
        {
            if name == slot && local_is_zero_or_false(Local(*v), defs) {
                return true;
            }
        }
    }
    false
}

/// `Local` folds to `0` or `false` (trial_div then-arm flag clear).
pub fn local_is_zero_or_false(v: Local, defs: &HashMap<u32, Value>) -> bool {
    const_of(v, defs) == Some(0) || matches!(defs.get(&v.0), Some(Value::Bool(false)))
}

/// Local is a `Rem` binary (nsw / Euclid peeps).
pub fn is_rem(l: Local, defs: &HashMap<u32, Value>) -> bool {
    matches!(defs.get(&l.0), Some(Value::Binary { op: BinOp::Rem, .. }))
}

/// Add/Sub that is not a unit latch increment (`i+1` / `1+i`).
pub fn is_nontrivial_add_or_sub(v: &Value, defs: &HashMap<u32, Value>) -> bool {
    matches!(
        v,
        Value::Binary {
            op: BinOp::Add | BinOp::Sub,
            ..
        } if !is_unit_inc_value(v, defs)
    )
}

/// Mul/Div, or nontrivial Add/Sub (dense_f64 elementwise disqualifiers).
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

/// Whether `block` assigns `name = (_ % _)` (Euclid `y = x % y` shape).
pub fn body_assigns_rem(block: &Block, name: &str, defs: &HashMap<u32, Value>) -> bool {
    for op in &block.ops {
        if let Op::Assign {
            name: n,
            value: Local(v),
        } = op
        {
            if n == name && matches!(defs.get(v), Some(Value::Binary { op: BinOp::Rem, .. })) {
                return true;
            }
        }
    }
    false
}

/// Whether `block` assigns `name = name + 1` / `1 + name` (unit induction step).
pub fn body_assigns_unit_inc(block: &Block, name: &str, defs: &HashMap<u32, Value>) -> bool {
    for op in &block.ops {
        if let Op::Assign {
            name: n,
            value: Local(v),
        } = op
        {
            if n == name && is_unit_inc(*v, name, defs) {
                return true;
            }
        }
    }
    false
}

/// `Name(iv) + 1` / `1 + Name(iv)` for any induction slot name.
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
    let one_l = const_of(*left, defs) == Some(1);
    let one_r = const_of(*right, defs) == Some(1);
    let name_l = name_of(*left, defs).is_some();
    let name_r = name_of(*right, defs).is_some();
    (name_l && one_r) || (name_r && one_l)
}

/// `(a % b) == 0` / `0 == (a % b)` → rem operands `(a, b)`.
pub fn rem_eq_zero_operands(cond: Local, defs: &HashMap<u32, Value>) -> Option<(Local, Local)> {
    let Value::Binary {
        op: BinOp::Eq,
        left,
        right,
        ..
    } = defs.get(&cond.0)?
    else {
        return None;
    };
    let zero_l = const_of(*left, defs) == Some(0);
    let zero_r = const_of(*right, defs) == Some(0);
    if !zero_l && !zero_r {
        return None;
    }
    let rem = if zero_l { *right } else { *left };
    let Value::Binary {
        op: BinOp::Rem,
        left: a,
        right: b,
        ..
    } = defs.get(&rem.0)?
    else {
        return None;
    };
    Some((*a, *b))
}

/// `(Name(a) % Name(b)) == 0` either operand order.
pub fn rem_eq_zero_names(
    cond: Local,
    a_name: &str,
    b_name: &str,
    defs: &HashMap<u32, Value>,
) -> bool {
    let Some((a, b)) = rem_eq_zero_operands(cond, defs) else {
        return false;
    };
    (name_of(a, defs).as_deref() == Some(a_name) && name_of(b, defs).as_deref() == Some(b_name))
        || (name_of(a, defs).as_deref() == Some(b_name)
            && name_of(b, defs).as_deref() == Some(a_name))
}

/// `(Name(name) % k) == 0` / `(k % Name(name)) == 0`.
pub fn is_name_rem_eq_const(cond: Local, name: &str, k: i64, defs: &HashMap<u32, Value>) -> bool {
    let Some((a, b)) = rem_eq_zero_operands(cond, defs) else {
        return false;
    };
    (name_of(a, defs).as_deref() == Some(name) && const_of(b, defs) == Some(k))
        || (name_of(b, defs).as_deref() == Some(name) && const_of(a, defs) == Some(k))
}

/// `Name(name) / k` / `k / Name(name)`.
pub fn is_name_div_const(dest: u32, name: &str, k: i64, defs: &HashMap<u32, Value>) -> bool {
    let Some(Value::Binary {
        op: BinOp::Div,
        left,
        right,
        ..
    }) = defs.get(&dest)
    else {
        return false;
    };
    (name_of(*left, defs).as_deref() == Some(name) && const_of(*right, defs) == Some(k))
        || (name_of(*right, defs).as_deref() == Some(name) && const_of(*left, defs) == Some(k))
}

/// `Name(name) * Name(name)` (nsw `iv * iv` under square bound).
pub fn is_name_mul_name(l: Local, name: &str, defs: &HashMap<u32, Value>) -> bool {
    let Some(Value::Binary {
        op: BinOp::Mul,
        left,
        right,
        ..
    }) = defs.get(&l.0)
    else {
        return false;
    };
    name_of(*left, defs).as_deref() == Some(name) && name_of(*right, defs).as_deref() == Some(name)
}

/// `c * nonneg` / `nonneg * c` with `1..=max_factor` (nsw Collatz `3*x`).
pub fn is_small_factor_mul_nonneg(
    l: Local,
    max_factor: i64,
    nonneg: &HashSet<u32>,
    defs: &HashMap<u32, Value>,
) -> bool {
    let Some(Value::Binary {
        op: BinOp::Mul,
        left,
        right,
        ..
    }) = defs.get(&l.0)
    else {
        return false;
    };
    let (lc, rc) = (const_of(*left, defs), const_of(*right, defs));
    matches!(
        (lc, rc),
        (Some(c), _) if (1..=max_factor).contains(&c) && nonneg.contains(&right.0)
    ) || matches!(
        (lc, rc),
        (_, Some(c)) if (1..=max_factor).contains(&c) && nonneg.contains(&left.0)
    )
}

/// `Name(name) * n` / `n * Name(name)`.
pub fn is_name_mul_const(l: Local, name: &str, n: i64, defs: &HashMap<u32, Value>) -> bool {
    let Some(Value::Binary {
        op: BinOp::Mul,
        left,
        right,
        ..
    }) = defs.get(&l.0)
    else {
        return false;
    };
    (name_of(*left, defs).as_deref() == Some(name) && const_of(*right, defs) == Some(n))
        || (name_of(*right, defs).as_deref() == Some(name) && const_of(*left, defs) == Some(n))
}

/// `(mul_k * Name(name)) + add_k` / `add_k + (mul_k * Name(name))`.
///
/// Collatz odd branch: `x = 3 * x + 1`.
pub fn is_name_mul_const_plus_const(
    dest: u32,
    name: &str,
    mul_k: i64,
    add_k: i64,
    defs: &HashMap<u32, Value>,
) -> bool {
    let Some(Value::Binary {
        op: BinOp::Add,
        left,
        right,
        ..
    }) = defs.get(&dest)
    else {
        return false;
    };
    let mul_l = if const_of(*right, defs) == Some(add_k) {
        *left
    } else if const_of(*left, defs) == Some(add_k) {
        *right
    } else {
        return false;
    };
    is_name_mul_const(mul_l, name, mul_k, defs)
}

/// Whether `block` assigns `name = name / k` / `k / name` (Collatz even branch).
pub fn body_assigns_name_div_const(
    block: &Block,
    name: &str,
    k: i64,
    defs: &HashMap<u32, Value>,
) -> bool {
    for op in &block.ops {
        if let Op::Assign {
            name: n,
            value: Local(v),
        } = op
        {
            if n == name && is_name_div_const(*v, name, k, defs) {
                return true;
            }
        }
    }
    false
}

/// Whether `block` assigns `name = (mul_k * name) + add_k` (or commute).
pub fn body_assigns_name_mul_const_plus_const(
    block: &Block,
    name: &str,
    mul_k: i64,
    add_k: i64,
    defs: &HashMap<u32, Value>,
) -> bool {
    for op in &block.ops {
        if let Op::Assign {
            name: n,
            value: Local(v),
        } = op
        {
            if n == name && is_name_mul_const_plus_const(*v, name, mul_k, add_k, defs) {
                return true;
            }
        }
    }
    false
}

/// `Name(name) + K` / `K + Name(name)` → const `K`.
pub fn is_name_add_const(dest: u32, name: &str, defs: &HashMap<u32, Value>) -> Option<i64> {
    let Value::Binary {
        op: BinOp::Add,
        left,
        right,
        ..
    } = defs.get(&dest)?
    else {
        return None;
    };
    if name_of(*left, defs).as_deref() == Some(name) {
        const_of(*right, defs)
    } else if name_of(*right, defs).as_deref() == Some(name) {
        const_of(*left, defs)
    } else {
        None
    }
}

/// `Name(a) + Name(b)` / `Name(b) + Name(a)`.
pub fn is_add_name_plus_name(dest: u32, a: &str, b: &str, defs: &HashMap<u32, Value>) -> bool {
    let Some(Value::Binary {
        op: BinOp::Add,
        left,
        right,
        ..
    }) = defs.get(&dest)
    else {
        return false;
    };
    let ln = name_of(*left, defs);
    let rn = name_of(*right, defs);
    (ln.as_deref() == Some(a) && rn.as_deref() == Some(b))
        || (ln.as_deref() == Some(b) && rn.as_deref() == Some(a))
}

/// `Name(s) + _` / `_ + Name(s)` → the other operand local.
pub fn add_name_other(dest: u32, s_name: &str, defs: &HashMap<u32, Value>) -> Option<Local> {
    let Value::Binary {
        op: BinOp::Add,
        left,
        right,
        ..
    } = defs.get(&dest)?
    else {
        return None;
    };
    if name_of(*left, defs).as_deref() == Some(s_name) {
        Some(*right)
    } else if name_of(*right, defs).as_deref() == Some(s_name) {
        Some(*left)
    } else {
        None
    }
}

/// `Name(s) + _` / `_ + Name(s)` (other operand unconstrained).
pub fn is_add_name_plus_any(dest: u32, s_name: &str, defs: &HashMap<u32, Value>) -> bool {
    add_name_other(dest, s_name, defs).is_some()
}

/// `term % m` with const `m >= 2` → `(numerator, m)`.
pub fn rem_const_mod(term: Local, defs: &HashMap<u32, Value>) -> Option<(Local, i64)> {
    let Value::Binary {
        op: BinOp::Rem,
        left: num,
        right: den,
        ..
    } = defs.get(&term.0)?
    else {
        return None;
    };
    let m = const_of(*den, defs).filter(|m| *m >= 2)?;
    Some((*num, m))
}

/// `acc = acc + (num % m)` with const `m >= 2` → `(num, m)`.
pub fn acc_add_rem_const_mod(
    dest: u32,
    acc: &str,
    defs: &HashMap<u32, Value>,
) -> Option<(Local, i64)> {
    let rem_l = add_name_other(dest, acc, defs)?;
    rem_const_mod(rem_l, defs)
}

/// `i*n + k + 1` affine index shape.
pub fn is_affine_ik1(l: Local, i: &str, k: &str, n: i64, defs: &HashMap<u32, Value>) -> bool {
    let Some(Value::Binary {
        op: BinOp::Add,
        left,
        right,
        ..
    }) = defs.get(&l.0)
    else {
        return false;
    };
    let (rest, one) = if const_of(*left, defs) == Some(1) {
        (*right, true)
    } else if const_of(*right, defs) == Some(1) {
        (*left, true)
    } else {
        return false;
    };
    if !one {
        return false;
    }
    let Some(Value::Binary {
        op: BinOp::Add,
        left: a,
        right: b,
        ..
    }) = defs.get(&rest.0)
    else {
        return false;
    };
    matches!(
        (
            is_name_mul_const(*a, i, n, defs),
            name_of(*b, defs).as_deref() == Some(k),
            is_name_mul_const(*b, i, n, defs),
            name_of(*a, defs).as_deref() == Some(k),
        ),
        (true, true, _, _) | (_, _, true, true)
    )
}

/// `k*n + j + 1` affine index shape.
pub fn is_affine_kj1(l: Local, k: &str, j: &str, n: i64, defs: &HashMap<u32, Value>) -> bool {
    let Some(Value::Binary {
        op: BinOp::Add,
        left,
        right,
        ..
    }) = defs.get(&l.0)
    else {
        return false;
    };
    let (rest, one) = if const_of(*left, defs) == Some(1) {
        (*right, true)
    } else if const_of(*right, defs) == Some(1) {
        (*left, true)
    } else {
        return false;
    };
    if !one {
        return false;
    }
    let Some(Value::Binary {
        op: BinOp::Add,
        left: a,
        right: b,
        ..
    }) = defs.get(&rest.0)
    else {
        return false;
    };
    matches!(
        (
            is_name_mul_const(*a, k, n, defs),
            name_of(*b, defs).as_deref() == Some(j),
            is_name_mul_const(*b, k, n, defs),
            name_of(*a, defs).as_deref() == Some(j),
        ),
        (true, true, _, _) | (_, _, true, true)
    )
}

/// Header result is `Name(d) * Name(d) {Le|Lt} bound` → `(d, bound, strict)` where
/// `strict` is true for `Lt`. Bound may be Const or Name (caller resolves).
pub fn header_name_sq_cmp(
    header: &Block,
    defs: &HashMap<u32, Value>,
) -> Option<(String, Local, bool)> {
    let res = header.result?;
    let Value::Binary {
        op, left, right, ..
    } = defs.get(&res.0)?
    else {
        return None;
    };
    let strict = match op {
        BinOp::Lt => true,
        BinOp::Le => false,
        _ => return None,
    };
    let Value::Binary {
        op: BinOp::Mul,
        left: a,
        right: b,
        ..
    } = defs.get(&left.0)?
    else {
        return None;
    };
    let da = name_of(*a, defs)?;
    let db = name_of(*b, defs)?;
    if da != db {
        return None;
    }
    Some((da, *right, strict))
}

/// Header result is `Name(d) * Name(d) <= Name(n)` (trial division bound).
pub fn header_name_sq_le_name(
    header: &Block,
    defs: &HashMap<u32, Value>,
) -> Option<(String, String)> {
    let (d, bound, strict) = header_name_sq_cmp(header, defs)?;
    if strict {
        return None;
    }
    let n = name_of(bound, defs)?;
    Some((d, n))
}

/// Leaf SSA defs for SR / NSW peeps (Int/Float/Name/Binary/Builtin; optionally AllocList).
///
/// Shared by codegen `nsw_iv` and opt `dense_f64_sr` (Todo: dense/nsw 缺共用 leaf defs).
pub fn collect_leaf_defs(body: &Block, include_alloc_list: bool) -> HashMap<u32, Value> {
    use crate::visit::for_each_block_dfs;
    let mut all_defs: HashMap<u32, Value> = HashMap::default();
    for_each_block_dfs(body, &mut |b| {
        for op in &b.ops {
            if let Op::Let { local, value, .. } = op {
                let take = matches!(
                    value,
                    Value::Int(_)
                        | Value::Float(_)
                        | Value::Name(_)
                        | Value::Binary { .. }
                        | Value::Builtin { .. }
                ) || (include_alloc_list && matches!(value, Value::AllocList { .. }));
                if take {
                    all_defs.insert(local.0, value.clone());
                }
            }
        }
    });
    all_defs
}

/// Whether any leaf def is a float literal within `1e-12` of `target`.
pub fn has_float_approx(defs: &HashMap<u32, Value>, target: f64) -> bool {
    defs.values().any(|v| match v {
        Value::Float(f) => (*f - target).abs() < 1e-12,
        _ => false,
    })
}

/// Whether any binary `op` has a float-literal operand within `1e-12` of `target`.
pub fn has_float_binop_with_const(defs: &HashMap<u32, Value>, op: BinOp, target: f64) -> bool {
    defs.values().any(|v| {
        let Value::Binary {
            op: bop,
            left,
            right,
            ..
        } = v
        else {
            return false;
        };
        if *bop != op {
            return false;
        }
        let lf = match defs.get(&left.0) {
            Some(Value::Float(f)) => Some(*f),
            _ => None,
        };
        let rf = match defs.get(&right.0) {
            Some(Value::Float(f)) => Some(*f),
            _ => None,
        };
        lf.is_some_and(|f| (f - target).abs() < 1e-12)
            || rf.is_some_and(|f| (f - target).abs() < 1e-12)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CoreBinOp as BinOp;
    use crate::{Block, Local, Value};

    #[test]
    fn unit_inc_and_header_lt_gt() {
        let mut defs = HashMap::default();
        defs.insert(0, Value::Name("i".into()));
        defs.insert(1, Value::Int(1));
        defs.insert(
            2,
            Value::Binary {
                op: BinOp::Add,
                left: Local(0),
                right: Local(1),
            },
        );
        assert!(is_unit_inc(2, "i", &defs));
        assert!(!is_unit_inc(2, "j", &defs));

        defs.insert(3, Value::Int(10));
        defs.insert(
            4,
            Value::Binary {
                op: BinOp::Lt,
                left: Local(0),
                right: Local(3),
            },
        );
        let header = Block {
            ops: vec![],
            result: Some(Local(4)),
        };
        assert_eq!(header_lt_const(&header, &defs), Some(("i".into(), 10)));

        defs.insert(
            5,
            Value::Binary {
                op: BinOp::Gt,
                left: Local(3),
                right: Local(0),
            },
        );
        let header_gt = Block {
            ops: vec![],
            result: Some(Local(5)),
        };
        assert_eq!(header_lt_const(&header_gt, &defs), Some(("i".into(), 10)));
    }
}
