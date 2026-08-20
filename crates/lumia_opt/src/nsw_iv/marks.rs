use lumia_core::CoreBinOp as BinOp;
use lumia_core::{
    collect_assigns, collect_iv_unit_step_dests, collect_name_loads_in_block, const_of,
    for_each_block_dfs, is_rem, is_small_factor_mul_nonneg, is_unit_inc, name_of, Block, Local,
    Value,
};
use lumia_syntax::Sym;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

use super::divisors::{collect_ge1_unit_slots, collect_unit_counter_slots};
use super::{NSW_ACC_BOUND_MAX, NSW_BOUND_MAX, NSW_REM_MOD_MAX, NSW_SMALL_FACTOR};

/// Mark `nonneg_iv ±/× const` when the IV has a known const upper `U` and
/// `U.checked_{add,mul}(c)` succeeds (`c ≥ 0`).
///
/// Uses the exclusive/inclusive loop upper stored in `iv_upper` (same map as
/// square / bounded-tree peeps). Sound and slightly conservative for exclusive
/// `< K` (actual max is `K-1`).
pub(super) fn mark_nonneg_iv_add_mul_bounded(
    all_defs: &HashMap<u32, Value>,
    nonneg_loads: &HashSet<u32>,
    iv_upper: &HashMap<Sym, i64>,
    out: &mut HashSet<u32>,
) {
    if iv_upper.is_empty() || nonneg_loads.is_empty() {
        return;
    }
    for (id, v) in all_defs {
        if out.contains(id) {
            continue;
        }
        let Value::Binary {
            op, left, right, ..
        } = v
        else {
            continue;
        };
        if !matches!(op, BinOp::Add | BinOp::Mul) {
            continue;
        }
        let mark = |iv_local: u32, c_local: u32| -> bool {
            if !nonneg_loads.contains(&iv_local) {
                return false;
            }
            let Some(c) = const_of(Local(c_local), all_defs) else {
                return false;
            };
            if c < 0 {
                return false;
            }
            let Some(name) = name_of(Local(iv_local), all_defs) else {
                return false;
            };
            let Some(&u) = iv_upper.get(&name) else {
                return false;
            };
            match op {
                BinOp::Add => u.checked_add(c).is_some(),
                BinOp::Mul => u.checked_mul(c).is_some(),
                _ => false,
            }
        };
        if mark(left.0, right.0) || mark(right.0, left.0) {
            out.insert(*id);
        }
    }
}

/// Mark `iv1 ±/× iv2` when both loads are proven nonnegative and the op fits i64.
pub(super) fn mark_bounded_nonneg_pair_add(
    all_defs: &HashMap<u32, Value>,
    nonneg_loads: &HashSet<u32>,
    iv_upper: &HashMap<Sym, i64>,
    out: &mut HashSet<u32>,
) {
    if iv_upper.is_empty() || nonneg_loads.is_empty() {
        return;
    }
    for (id, v) in all_defs {
        if out.contains(id) {
            continue;
        }
        let Value::Binary {
            op, left, right, ..
        } = v
        else {
            continue;
        };
        if !matches!(op, BinOp::Add | BinOp::Mul) {
            continue;
        }
        let fits = |l: u32, r: u32| -> bool {
            if !nonneg_loads.contains(&l) || !nonneg_loads.contains(&r) {
                return false;
            }
            let Some(n1) = name_of(Local(l), all_defs) else {
                return false;
            };
            let Some(n2) = name_of(Local(r), all_defs) else {
                return false;
            };
            let Some(&u1) = iv_upper.get(&n1) else {
                return false;
            };
            let Some(&u2) = iv_upper.get(&n2) else {
                return false;
            };
            match op {
                BinOp::Add => u1.checked_add(u2).is_some(),
                BinOp::Mul => u1.checked_mul(u2).is_some(),
                _ => false,
            }
        };
        if fits(left.0, right.0) || fits(right.0, left.0) {
            out.insert(*id);
        }
    }
}

/// Mark `a + b` when both operands are nonnegative Int literals and the sum
/// fits in signed i64 (checked at mark time).
pub(super) fn mark_nonneg_const_add(all_defs: &HashMap<u32, Value>, out: &mut HashSet<u32>) {
    for (id, v) in all_defs {
        if out.contains(id) {
            continue;
        }
        let Value::Binary {
            op: BinOp::Add,
            left,
            right,
            ..
        } = v
        else {
            continue;
        };
        let (Some(a), Some(b)) = (const_of(*left, all_defs), const_of(*right, all_defs)) else {
            continue;
        };
        if a >= 0 && b >= 0 && a.checked_add(b).is_some() {
            out.insert(*id);
        }
    }
}

/// Mark `a * b` when both operands are nonnegative Int literals and the product
/// fits in signed i64.
pub(super) fn mark_nonneg_const_mul(all_defs: &HashMap<u32, Value>, out: &mut HashSet<u32>) {
    for (id, v) in all_defs {
        if out.contains(id) {
            continue;
        }
        let Value::Binary {
            op: BinOp::Mul,
            left,
            right,
            ..
        } = v
        else {
            continue;
        };
        let (Some(a), Some(b)) = (const_of(*left, all_defs), const_of(*right, all_defs)) else {
            continue;
        };
        if a >= 0 && b >= 0 && a.checked_mul(b).is_some() {
            out.insert(*id);
        }
    }
}

/// Mark `a - b` when both operands are nonnegative (nonneg IV load or `Int ≥ 0`).
///
/// For `x,y ∈ [0, 2⁶³)`, `x - y` fits in signed i64 (min result `0 - (2⁶³-1) = 1-2⁶³`).
pub(super) fn mark_nonneg_sub(
    all_defs: &HashMap<u32, Value>,
    nonneg_loads: &HashSet<u32>,
    out: &mut HashSet<u32>,
) {
    fn is_nonneg(id: u32, nonneg_loads: &HashSet<u32>, all_defs: &HashMap<u32, Value>) -> bool {
        if nonneg_loads.contains(&id) {
            return true;
        }
        matches!(all_defs.get(&id), Some(Value::Int(n)) if *n >= 0)
    }
    for (id, v) in all_defs {
        if out.contains(id) {
            continue;
        }
        let Value::Binary {
            op: BinOp::Sub,
            left,
            right,
            ..
        } = v
        else {
            continue;
        };
        if is_nonneg(left.0, nonneg_loads, all_defs) && is_nonneg(right.0, nonneg_loads, all_defs) {
            out.insert(*id);
        }
    }
}

pub(super) fn mark_unit_steps(
    block: &Block,
    iv: &str,
    all_defs: &HashMap<u32, Value>,
    out: &mut HashSet<u32>,
) {
    collect_iv_unit_step_dests(block, iv, all_defs, out);
}

pub(super) fn mark_small_factor_muls(
    all_defs: &HashMap<u32, Value>,
    nonneg_loads: &HashSet<u32>,
    iv_upper: &HashMap<Sym, i64>,
    out: &mut HashSet<u32>,
) {
    for (id, v) in all_defs {
        if !is_small_factor_mul_nonneg(Local(*id), NSW_SMALL_FACTOR, nonneg_loads, all_defs) {
            continue;
        }
        // Open exclusive uppers seed `iv_upper = MAX-1`; `c * i` is unsound there.
        // Allow: no upper recorded (lower-bound nonneg, e.g. Collatz `x > 1`), or
        // a modest const upper ≤ NSW_IV_UPPER_MAX.
        let Value::Binary { left, right, .. } = v else {
            continue;
        };
        let iv_name = [left, right].into_iter().find_map(|side| {
            if nonneg_loads.contains(&side.0) {
                name_of(*side, all_defs)
            } else {
                None
            }
        });
        let ok_upper = match iv_name {
            Some(n) => match iv_upper.get(&n) {
                None => true,
                Some(&u) => u <= super::NSW_IV_UPPER_MAX,
            },
            None => true,
        };
        if ok_upper {
            out.insert(*id);
        }
    }
    // Close `3*x + 1` (Collatz) — add/sub of a just-marked factor mul and tiny const.
    let mut changed = true;
    while changed {
        changed = false;
        for (id, v) in all_defs {
            if out.contains(id) {
                continue;
            }
            let Value::Binary {
                op, left, right, ..
            } = v
            else {
                continue;
            };
            if !matches!(op, BinOp::Add | BinOp::Sub) {
                continue;
            }
            let l_ok = out.contains(&left.0)
                && const_of(*right, all_defs).is_some_and(|c| (0..=NSW_SMALL_FACTOR).contains(&c));
            let r_ok = out.contains(&right.0)
                && const_of(*left, all_defs).is_some_and(|c| (0..=NSW_SMALL_FACTOR).contains(&c));
            if l_ok || r_ok {
                out.insert(*id);
                changed = true;
            }
        }
    }
}

/// Mark `a / d` when `d` is a safe divisor (≠0,≠−1 / ge1 unit slot).
/// Signed `i64` div overflows only for `MIN/−1`; safe divisors exclude −1.
pub(super) fn mark_safe_div_bins(
    body: &Block,
    all_defs: &HashMap<u32, Value>,
    out: &mut HashSet<u32>,
) -> HashSet<u32> {
    let ge1 = collect_ge1_unit_slots(body, all_defs);
    let mut div_ok: HashSet<u32> = HashSet::default();
    for (id, v) in all_defs {
        if let Value::Binary {
            op: BinOp::Div,
            right,
            ..
        } = v
        {
            let safe_r = match all_defs.get(&right.0) {
                Some(Value::Int(n)) if *n != 0 && *n != -1 => true,
                Some(Value::Name(n)) if ge1.contains(n.as_str()) => true,
                _ => false,
            };
            if safe_r {
                div_ok.insert(*id);
                out.insert(*id);
            }
        }
    }
    div_ok
}

/// Mark `e % C` when `C` is a small positive const (rem ∈ `[0, C)` fits i64).
pub(super) fn mark_bounded_rem_bins(
    all_defs: &HashMap<u32, Value>,
    out: &mut HashSet<u32>,
) -> HashSet<u32> {
    let mut rem_ok: HashSet<u32> = HashSet::default();
    for (id, v) in all_defs {
        if let Value::Binary {
            op: BinOp::Rem,
            left: _,
            right,
            ..
        } = v
        {
            let c = const_of(*right, all_defs);
            if c.is_some_and(|c| (2..=NSW_REM_MOD_MAX).contains(&c)) {
                // Rem with positive const modulus ≥2 does not overflow for any dividend.
                rem_ok.insert(*id);
                out.insert(*id);
            }
        }
    }
    rem_ok
}

/// Mark `Name(acc) + (a / d)` when `d` is a safe divisor (≠0,≠−1 / ge1 unit slot).
/// Signed `i64` div overflows only for `MIN/−1`; safe divisors exclude −1.
pub(super) fn mark_acc_plus_safe_div(
    body: &Block,
    all_defs: &HashMap<u32, Value>,
    out: &mut HashSet<u32>,
) {
    let div_ok = mark_safe_div_bins(body, all_defs, out);
    if div_ok.is_empty() {
        return;
    }
    let div_accs = rem_accumulator_slots(body, all_defs, &div_ok);
    let mut changed = true;
    while changed {
        changed = false;
        for (id, v) in all_defs {
            if out.contains(id) {
                continue;
            }
            let Value::Binary {
                op: BinOp::Add,
                left,
                right,
                ..
            } = v
            else {
                continue;
            };
            let l_div = div_ok.contains(&left.0);
            let r_div = div_ok.contains(&right.0);
            let l_acc = out.contains(&left.0)
                || name_of(*left, all_defs).is_some_and(|n| div_accs.contains(&n));
            let r_acc = out.contains(&right.0)
                || name_of(*right, all_defs).is_some_and(|n| div_accs.contains(&n));
            if (l_div && r_acc) || (r_div && l_acc) {
                out.insert(*id);
                changed = true;
            }
        }
    }
}

/// Mark `Name(acc) + (e % C)` (and reverse) as NSW when `C` is a small positive const
/// and `e % C` (or `e`) is already in `out`. Safe under const IV bounds ≤ [`NSW_BOUND_MAX`].
///
/// Also bootstraps rem-accumulator slots: init `0`, assigns only `acc += rem`
/// (so `var s = 0; s = s + (e % C)` marks the add without requiring `s` already NSW).
pub(super) fn mark_acc_plus_bounded_rem(
    body: &Block,
    all_defs: &HashMap<u32, Value>,
    out: &mut HashSet<u32>,
) {
    let rem_ok = mark_bounded_rem_bins(all_defs, out);
    if rem_ok.is_empty() {
        return;
    }
    let rem_accs = rem_accumulator_slots(body, all_defs, &rem_ok);
    let mut changed = true;
    while changed {
        changed = false;
        for (id, v) in all_defs {
            if out.contains(id) {
                continue;
            }
            let Value::Binary {
                op: BinOp::Add,
                left,
                right,
                ..
            } = v
            else {
                continue;
            };
            let l_rem =
                rem_ok.contains(&left.0) || (out.contains(&left.0) && is_rem(*left, all_defs));
            let r_rem =
                rem_ok.contains(&right.0) || (out.contains(&right.0) && is_rem(*right, all_defs));
            let l_acc = out.contains(&left.0)
                || name_of(*left, all_defs).is_some_and(|n| rem_accs.contains(&n));
            let r_acc = out.contains(&right.0)
                || name_of(*right, all_defs).is_some_and(|n| rem_accs.contains(&n));
            if (l_rem && r_acc) || (r_rem && l_acc) {
                out.insert(*id);
                changed = true;
            }
        }
    }
}

/// Slots init to `0` whose other assigns are only `self + rem` / `rem + self`.
fn rem_accumulator_slots(
    body: &Block,
    all_defs: &HashMap<u32, Value>,
    rem_ok: &HashSet<u32>,
) -> HashSet<Sym> {
    let mut assigns = HashMap::default();
    collect_assigns(body, &mut assigns);
    let mut out = HashSet::default();
    'slots: for (name, vals) in assigns {
        let mut has_zero = false;
        if vals.is_empty() {
            continue;
        }
        for v in vals {
            match all_defs.get(&v.0) {
                Some(Value::Int(0)) => has_zero = true,
                Some(Value::Binary {
                    op: BinOp::Add,
                    left,
                    right,
                    ..
                }) => {
                    let ln = name_of(*left, all_defs);
                    let rn = name_of(*right, all_defs);
                    let l_self = ln.as_deref() == Some(name.as_str());
                    let r_self = rn.as_deref() == Some(name.as_str());
                    let l_rem = rem_ok.contains(&left.0) || is_rem(*left, all_defs);
                    let r_rem = rem_ok.contains(&right.0) || is_rem(*right, all_defs);
                    if !((l_self && r_rem) || (r_self && l_rem)) {
                        continue 'slots;
                    }
                }
                _ => continue 'slots,
            }
        }
        if has_zero {
            out.insert(name);
        }
    }
    out
}

/// Mark unit-counter self-incs (`steps += 1`) and `Name(acc) + counter` under large
/// const-bounded outer loops (Collatz `total += steps`).
pub(super) fn mark_acc_plus_unit_counter(
    body: &Block,
    all_defs: &HashMap<u32, Value>,
    out: &mut HashSet<u32>,
) {
    let counters = collect_unit_counter_slots(body, all_defs);
    if counters.is_empty() {
        return;
    }
    // Bootstrap: the `+= 1` Binary itself is NSW (counter ≤ trip count).
    for (id, _) in all_defs {
        for name in &counters {
            if is_unit_inc(*id, name, all_defs) {
                out.insert(*id);
            }
        }
    }
    let acc_slots = unit_counter_acc_slots(body, all_defs, &counters);
    let mut changed = true;
    while changed {
        changed = false;
        for (id, v) in all_defs {
            if out.contains(id) {
                continue;
            }
            let Value::Binary {
                op: BinOp::Add,
                left,
                right,
                ..
            } = v
            else {
                continue;
            };
            let ln = name_of(*left, all_defs);
            let rn = name_of(*right, all_defs);
            let l_ctr = ln.as_ref().is_some_and(|n| counters.contains(n));
            let r_ctr = rn.as_ref().is_some_and(|n| counters.contains(n));
            let l_acc = out.contains(&left.0) || ln.as_ref().is_some_and(|n| acc_slots.contains(n));
            let r_acc =
                out.contains(&right.0) || rn.as_ref().is_some_and(|n| acc_slots.contains(n));
            if (l_ctr && r_acc) || (r_ctr && l_acc) {
                out.insert(*id);
                changed = true;
            }
        }
    }
}

/// Accumulators init `0` whose other assigns are only `self + unit_counter`.
fn unit_counter_acc_slots(
    body: &Block,
    all_defs: &HashMap<u32, Value>,
    counters: &HashSet<Sym>,
) -> HashSet<Sym> {
    let mut assigns = HashMap::default();
    collect_assigns(body, &mut assigns);
    let mut out = HashSet::default();
    'slots: for (name, vals) in assigns {
        if counters.contains(&name) {
            continue;
        }
        let mut has_zero = false;
        if vals.is_empty() {
            continue;
        }
        for v in vals {
            match all_defs.get(&v.0) {
                Some(Value::Int(0)) => has_zero = true,
                Some(Value::Binary {
                    op: BinOp::Add,
                    left,
                    right,
                    ..
                }) => {
                    let ln = name_of(*left, all_defs);
                    let rn = name_of(*right, all_defs);
                    let l_self = ln.as_deref() == Some(name.as_str());
                    let r_self = rn.as_deref() == Some(name.as_str());
                    let l_ctr = ln.as_ref().is_some_and(|n| counters.contains(n));
                    let r_ctr = rn.as_ref().is_some_and(|n| counters.contains(n));
                    if !((l_self && r_ctr) || (r_self && l_ctr)) {
                        continue 'slots;
                    }
                }
                _ => continue 'slots,
            }
        }
        if has_zero {
            out.insert(name);
        }
    }
    out
}

/// Close Add/Sub/Mul/Rem under IV loads + small consts when some exclusive bound is const.
pub(super) fn mark_bounded_arith_tree(
    body: &Block,
    ivs: &HashSet<Sym>,
    bound: i64,
    all_defs: &HashMap<u32, Value>,
    nonneg_loads: &HashSet<u32>,
    out: &mut HashSet<u32>,
) {
    let mut seed: HashSet<u32> = out.clone();
    let seed_names: HashSet<Sym> = ivs.clone();
    let const_lim = bound
        .saturating_mul(bound)
        .min(NSW_BOUND_MAX.saturating_mul(NSW_BOUND_MAX));
    for (id, v) in all_defs {
        match v {
            Value::Int(n) if *n >= 0 && *n <= const_lim => {
                seed.insert(*id);
            }
            Value::Name(n) if ivs.contains(n.as_str()) => {
                seed.insert(*id);
            }
            _ => {}
        }
    }
    for_each_block_dfs(body, &mut |b| {
        collect_name_loads_in_block(b, ivs, &mut seed);
    });
    mark_small_factor_muls(all_defs, nonneg_loads, &HashMap::default(), &mut seed);
    for id in &seed {
        if all_defs
            .get(id)
            .is_some_and(|v| matches!(v, Value::Binary { .. }))
        {
            out.insert(*id);
        }
    }

    let allow_acc = bound <= NSW_ACC_BOUND_MAX;
    let in_seed = |id: u32, seed: &HashSet<u32>, names: &HashSet<Sym>| {
        seed.contains(&id) || name_of(Local(id), all_defs).is_some_and(|n| names.contains(&n))
    };
    let mut changed = true;
    while changed {
        changed = false;
        for (id, v) in all_defs {
            if seed.contains(id) {
                continue;
            }
            let Value::Binary {
                op, left, right, ..
            } = v
            else {
                continue;
            };
            if !matches!(op, BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Rem) {
                continue;
            }
            let l = in_seed(left.0, &seed, &seed_names);
            let r = in_seed(right.0, &seed, &seed_names);
            let both = l && r;
            let rem_ok =
                matches!(op, BinOp::Rem) && l && const_of(*right, all_defs).is_some_and(|c| c > 1);
            if both || rem_ok {
                seed.insert(*id);
                out.insert(*id);
                changed = true;
            }
        }
    }
    // `Name(acc) + nsw` under small bounds (matmul `cell += product`): bootstrap
    // slots init 0 whose assigns are only `self + NSW` — do not pull `acc` into
    // the mul seed (would NSW `acc * …`).
    if allow_acc {
        mark_tree_acc_plus_nsw(body, all_defs, out);
    }
}

/// Accumulators init `0` whose other assigns are only `self + X` with `X` already NSW.
/// Marks those Add locals (matmul `cell += (i*n+k+1)*(…)` under `n ≤ NSW_ACC_BOUND_MAX`).
fn mark_tree_acc_plus_nsw(body: &Block, all_defs: &HashMap<u32, Value>, out: &mut HashSet<u32>) {
    let accs = tree_acc_slots(body, all_defs, out);
    if accs.is_empty() {
        return;
    }
    let mut changed = true;
    while changed {
        changed = false;
        for (id, v) in all_defs {
            if out.contains(id) {
                continue;
            }
            let Value::Binary {
                op: BinOp::Add,
                left,
                right,
                ..
            } = v
            else {
                continue;
            };
            let ln = name_of(*left, all_defs);
            let rn = name_of(*right, all_defs);
            let l_acc = ln.as_ref().is_some_and(|n| accs.contains(n));
            let r_acc = rn.as_ref().is_some_and(|n| accs.contains(n));
            let l_nsw = out.contains(&left.0);
            let r_nsw = out.contains(&right.0);
            if (l_acc && r_nsw) || (r_acc && l_nsw) {
                out.insert(*id);
                changed = true;
            }
        }
    }
}

/// Slots init `0`; every other assign is `self + nsw` / `nsw + self` (`nsw` ∈ `out`).
fn tree_acc_slots(
    body: &Block,
    all_defs: &HashMap<u32, Value>,
    nsw: &HashSet<u32>,
) -> HashSet<Sym> {
    let mut assigns = HashMap::default();
    collect_assigns(body, &mut assigns);
    let mut out = HashSet::default();
    'slots: for (name, vals) in assigns {
        let mut has_zero = false;
        if vals.is_empty() {
            continue;
        }
        for v in vals {
            match all_defs.get(&v.0) {
                Some(Value::Int(0)) => has_zero = true,
                Some(Value::Binary {
                    op: BinOp::Add,
                    left,
                    right,
                    ..
                }) => {
                    let ln = name_of(*left, all_defs);
                    let rn = name_of(*right, all_defs);
                    let l_self = ln.as_deref() == Some(name.as_str());
                    let r_self = rn.as_deref() == Some(name.as_str());
                    let l_nsw = nsw.contains(&left.0);
                    let r_nsw = nsw.contains(&right.0);
                    if !((l_self && r_nsw) || (r_self && l_nsw)) {
                        continue 'slots;
                    }
                }
                _ => continue 'slots,
            }
        }
        if has_zero {
            out.insert(name);
        }
    }
    out
}
