//! Mark loop induction updates — and const-bounded IV arith trees — as NSW-safe.
//!
//! ## Unit steps
//! - Strict `<`/`>`: `iv = iv ± 1` cannot overflow signed i64.
//! - Inclusive `<=`/`>=`: same only when the bound is a **known constant**
//!   (unknown `n` in `i <= n` may be `i64::MAX`).
//!
//! ## Bounded trees
//! With a small const bound (`1..=NSW_BOUND_MAX`), Add/Sub/Mul(/Rem) closed under
//! Accumulators: `Name(acc) + nsw` is included when the bound is ≤ `NSW_ACC_BOUND_MAX`.
//! With a larger const IV upper (`≤ NSW_IV_UPPER_MAX`), `acc += unit_counter` is
//! also NSW when the addend slot is only ever `0` / `+= 1` (Collatz `total += steps`).
//!
//! ## Extra peeps
//! - `c * nonneg_iv` for tiny `|c|` (Collatz `3*x`)
//! - `iv * iv` under `iv*iv ≤ C` (or ≤ outer const-bounded IV) headers
//! - Fib-style `match { 0|1 → …; _ → …(n-1)/(n-2) }`: mark `n-1`/`n-2` NSW
//!   in the residual arm (add of recursive results stays checked — fib(93)+ overflows)

use lumia_core::{
    collect_assigns, collect_leaf_defs as core_collect_leaf_defs, const_of, for_each_block_dfs,
    header_ge_const, header_gt_const, is_name_mul_name, is_rem, is_small_factor_mul_nonneg,
    is_unit_inc, is_unit_step, name_of, Block, Local, Op, Value,
};
use lumia_core::CoreBinOp as BinOp;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// Max loop bound for IV×bound arith trees (products must fit i64 with margin).
const NSW_BOUND_MAX: i64 = 30_000;
/// Cap for recording IV uppers used only to prove `d*d ≤ n` NSW.
const NSW_IV_UPPER_MAX: i64 = 1_000_000;
/// Tighter cap so `acc += O(B^4)` over `O(B^3)` iters still fits i64 (matmul-style).
const NSW_ACC_BOUND_MAX: i64 = 2_000;
/// `|c|` for `c * nonneg_iv` NSW peep (Collatz `3*x`).
const NSW_SMALL_FACTOR: i64 = 16;
/// `acc += (… % C)` with `C ≤ this` is NSW under const-bounded IVs (`≤ NSW_BOUND_MAX`):
/// rem ∈ `[0,C)` so `B²·C` fits i64 for `B ≤ 30_000`, `C ≤ 1_000_000`.
const NSW_REM_MOD_MAX: i64 = 1_000_000;

/// Locals that are `Binary` Add/Sub/Mul(/Rem) results proven safe for NSW emission.
#[cfg(test)]
pub(crate) fn collect_nsw_binop_locals(body: &Block) -> HashSet<u32> {
    analyze_nsw(body).nsw_binop_locals
}

/// Function-wide NSW / IV peep facts — computed once per function emit.
#[derive(Debug, Default, Clone)]
pub(crate) struct NswFacts {
    pub nsw_binop_locals: HashSet<u32>,
    pub safe_divisor_locals: HashSet<u32>,
    pub nonneg_iv_load_locals: HashSet<u32>,
    pub leaf_defs: HashMap<u32, Value>,
}

/// Run all NSW / leaf analyses for `body` in one pass bundle.
pub(crate) fn analyze_nsw(body: &Block) -> NswFacts {
    let leaf_defs = core_collect_leaf_defs(body, false);
    let nonneg_iv_load_locals = collect_nonneg_iv_load_locals(body);
    let safe_divisor_locals = collect_safe_divisor_locals(body);
    let nsw_binop_locals = collect_nsw_binop_locals_inner(body, &leaf_defs, &nonneg_iv_load_locals);
    NswFacts {
        nsw_binop_locals,
        safe_divisor_locals,
        nonneg_iv_load_locals,
        leaf_defs,
    }
}

fn collect_nsw_binop_locals_inner(
    body: &Block,
    all_defs: &HashMap<u32, Value>,
    nonneg_loads: &HashSet<u32>,
) -> HashSet<u32> {
    let mut out = HashSet::default();
    let mut bounded_ivs: HashSet<String> = HashSet::default();
    let mut iv_upper: HashMap<String, i64> = HashMap::default();
    let mut max_bound: i64 = 0;

    // Pass 1: unit steps + collect const-bounded IV uppers.
    for_each_block_dfs(body, &mut |b| {
        for op in &b.ops {
            if let Op::Let {
                value:
                    Value::Loop {
                        header,
                        body,
                        latch,
                    },
                ..
            } = op
            {
                let info = iv_bound_info(header, &all_defs);
                let unit_ok = info.strict
                    || info
                        .bound_const
                        .is_some_and(|b| (0..=i64::MAX - 2).contains(&b));
                if unit_ok {
                    for name in &info.ivs {
                        mark_unit_steps(body, name, &all_defs, &mut out);
                        mark_unit_steps(latch, name, &all_defs, &mut out);
                    }
                    // Do **not** NSW-mark arbitrary `x = x ± 1` in the body: a separate
                    // counter can still overflow while the IV stays in range.
                }
                if let Some(b) = info.bound_const {
                    if (1..=NSW_IV_UPPER_MAX).contains(&b) {
                        for name in &info.ivs {
                            let e = iv_upper.entry(name.clone()).or_insert(b);
                            *e = (*e).max(b);
                        }
                    }
                    if (1..=NSW_BOUND_MAX).contains(&b) {
                        bounded_ivs.extend(info.ivs.iter().cloned());
                        max_bound = max_bound.max(b);
                    }
                }
            }
        }
    });

    // Pass 2: `d*d ≤ C` / `d*d ≤ bounded_iv` — mark square + `d = d+1`.
    for_each_block_dfs(body, &mut |b| {
        for op in &b.ops {
            if let Op::Let {
                value:
                    Value::Loop {
                        header,
                        body,
                        latch,
                    },
                ..
            } = op
            {
                if let Some((iv, c)) = square_bound(header, &all_defs, &iv_upper) {
                    if (1..=NSW_IV_UPPER_MAX).contains(&c) {
                        mark_unit_steps(body, &iv, &all_defs, &mut out);
                        mark_unit_steps(latch, &iv, &all_defs, &mut out);
                        mark_square_mul(body, latch, &iv, &all_defs, &mut out);
                        let d_max = ((c as f64).sqrt().floor() as i64) + 2;
                        let e = iv_upper.entry(iv.clone()).or_insert(d_max);
                        *e = (*e).max(d_max);
                        if (1..=NSW_BOUND_MAX).contains(&c) {
                            bounded_ivs.insert(iv);
                            max_bound = max_bound.max(d_max);
                        }
                    }
                }
            }
        }
    });

    if !bounded_ivs.is_empty() {
        mark_bounded_arith_tree(
            body,
            &bounded_ivs,
            max_bound,
            &all_defs,
            &nonneg_loads,
            &mut out,
        );
        // Even when bound > NSW_ACC_BOUND_MAX, `s += (e % C)` is safe: rem < C.
        mark_acc_plus_bounded_rem(&all_defs, &mut out);
    }
    mark_small_factor_muls(&all_defs, &nonneg_loads, &mut out);

    // `total += steps` under a large const-bounded outer IV: RHS is a unit
    // counter (init 0, only +=1). Collatz max steps per n is tiny vs i64.
    let max_upper = iv_upper.values().copied().max().unwrap_or(0);
    if (1..=NSW_IV_UPPER_MAX).contains(&max_upper) {
        mark_acc_plus_unit_counter(body, &all_defs, &mut out);
    }

    // Fib-style `n-1`/`n-2` after match 0|1 is **not** NSW: residual includes
    // `n = i64::MIN` where `n-1` overflows. Keep checked sub there.
    out
}

struct IvBoundInfo {
    ivs: HashSet<String>,
    bound_const: Option<i64>,
    /// True for `<` / `>` (unit ±1 always NSW-safe).
    strict: bool,
}

/// IV names + optional **upper** const bound from `<`/`>`/`<=`/`>=`.
///
/// Only the induction-side `Name` is recorded (never the bound variable).
/// `bound_const` is set only when the constant is an exclusive/inclusive **upper**
/// bound (`i < K` / `K > i`), not a lower bound (`i > K`), so mul/acc trees stay sound.
fn iv_bound_info(header: &Block, all_defs: &HashMap<u32, Value>) -> IvBoundInfo {
    let empty = IvBoundInfo {
        ivs: HashSet::default(),
        bound_const: None,
        strict: false,
    };
    let Some(res) = header.result else {
        return empty;
    };
    let Some(Value::Binary {
        op, left, right, ..
    }) = all_defs.get(&res.0)
    else {
        return empty;
    };
    let strict = matches!(op, BinOp::Lt | BinOp::Gt);
    if !strict && !matches!(op, BinOp::Le | BinOp::Ge) {
        return empty;
    }
    let l_name = name_of(*left, all_defs);
    let r_name = name_of(*right, all_defs);
    let l_c = const_of(*left, all_defs);
    let r_c = const_of(*right, all_defs);
    let (iv, bound_const) = match op {
        // `iv < K` / `iv <= K` — K is an upper bound.
        BinOp::Lt | BinOp::Le if r_c.is_some() && l_name.is_some() => (l_name, r_c),
        // `K > iv` / `K >= iv` — K is an upper bound on iv.
        BinOp::Gt | BinOp::Ge if l_c.is_some() && r_name.is_some() => (r_name, l_c),
        // `iv < n` / `n > iv` with non-const bound: IV only, no const upper.
        BinOp::Lt | BinOp::Le if l_name.is_some() => (l_name, None),
        BinOp::Gt | BinOp::Ge if r_name.is_some() => (r_name, None),
        // Lower-bound forms (`iv > K`) — unit ±1 on iv is still NSW under strict
        // compares, but K must not seed bounded arith trees.
        BinOp::Gt | BinOp::Ge if l_name.is_some() && r_c.is_some() => (l_name, None),
        BinOp::Lt | BinOp::Le if r_name.is_some() && l_c.is_some() => (r_name, None),
        _ => return empty,
    };
    let mut ivs = HashSet::default();
    if let Some(n) = iv {
        ivs.insert(n);
    }
    IvBoundInfo {
        ivs,
        bound_const,
        strict,
    }
}

/// `Name(iv) * Name(iv) ≤ Const` or `≤ Name(bounded)` (isPrime trial loop).
fn square_bound(
    header: &Block,
    all_defs: &HashMap<u32, Value>,
    iv_upper: &HashMap<String, i64>,
) -> Option<(String, i64)> {
    let (iv, bound, _strict) = lumia_core::header_name_sq_cmp(header, all_defs)?;
    let c = const_of(bound, all_defs)
        .or_else(|| name_of(bound, all_defs).and_then(|n| iv_upper.get(&n).copied()))?;
    Some((iv, c))
}

fn mark_square_mul(
    body: &Block,
    latch: &Block,
    iv: &str,
    all_defs: &HashMap<u32, Value>,
    out: &mut HashSet<u32>,
) {
    let _ = (body, latch); // callers pass loop regions; mul may live in header via all_defs.
    for id in all_defs.keys() {
        if is_name_mul_name(Local(*id), iv, all_defs) {
            out.insert(*id);
        }
    }
}

fn mark_unit_steps(
    block: &Block,
    iv: &str,
    all_defs: &HashMap<u32, Value>,
    out: &mut HashSet<u32>,
) {
    for_each_block_dfs(block, &mut |b| {
        for op in &b.ops {
            let Op::Assign {
                name,
                value: Local(dest),
            } = op
            else {
                continue;
            };
            if name != iv {
                continue;
            }
            if is_unit_step(*dest, name, all_defs) {
                out.insert(*dest);
            }
        }
    });
}

fn mark_small_factor_muls(
    all_defs: &HashMap<u32, Value>,
    nonneg_loads: &HashSet<u32>,
    out: &mut HashSet<u32>,
) {
    for id in all_defs.keys() {
        if is_small_factor_mul_nonneg(Local(*id), NSW_SMALL_FACTOR, nonneg_loads, all_defs) {
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

/// Slots that are only ever assigned `0` or `self + 1` (Collatz `steps`, etc.).
fn collect_unit_counter_slots(body: &Block, all_defs: &HashMap<u32, Value>) -> HashSet<String> {
    let mut assigns = HashMap::default();
    collect_assigns(body, &mut assigns);
    let mut out = HashSet::default();
    for (name, vals) in assigns {
        let mut has_zero = false;
        let mut ok = !vals.is_empty();
        for v in vals {
            match all_defs.get(&v.0) {
                Some(Value::Int(0)) => has_zero = true,
                _ if is_unit_inc(v.0, name.as_str(), all_defs) => {}
                _ => {
                    ok = false;
                    break;
                }
            }
        }
        if ok && has_zero {
            out.insert(name);
        }
    }
    out
}

/// Mark `Name(acc) + (e % C)` (and reverse) as NSW when `C` is a small positive const
/// and `e % C` (or `e`) is already in `out`. Safe under const IV bounds ≤ [`NSW_BOUND_MAX`].
fn mark_acc_plus_bounded_rem(all_defs: &HashMap<u32, Value>, out: &mut HashSet<u32>) {
    let mut rem_ok: HashSet<u32> = HashSet::default();
    for (id, v) in all_defs {
        if let Value::Binary {
            op: BinOp::Rem,
            left,
            right,
            ..
        } = v
        {
            let c = const_of(*right, all_defs);
            if c.is_some_and(|c| (2..=NSW_REM_MOD_MAX).contains(&c))
                && (out.contains(&left.0) || out.contains(id))
            {
                rem_ok.insert(*id);
                out.insert(*id);
            }
        }
    }
    if rem_ok.is_empty() {
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
            let l_rem =
                rem_ok.contains(&left.0) || (out.contains(&left.0) && is_rem(*left, all_defs));
            let r_rem =
                rem_ok.contains(&right.0) || (out.contains(&right.0) && is_rem(*right, all_defs));
            // Acc side must already be NSW-proven — not an arbitrary `Name`.
            let l_acc = out.contains(&left.0);
            let r_acc = out.contains(&right.0);
            if (l_rem && r_acc) || (r_rem && l_acc) {
                out.insert(*id);
                changed = true;
            }
        }
    }
}

fn mark_acc_plus_unit_counter(
    body: &Block,
    all_defs: &HashMap<u32, Value>,
    out: &mut HashSet<u32>,
) {
    let counters = collect_unit_counter_slots(body, all_defs);
    if counters.is_empty() {
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
            let l_ctr = ln.as_ref().is_some_and(|n| counters.contains(n));
            let r_ctr = rn.as_ref().is_some_and(|n| counters.contains(n));
            // Other side must already be NSW (not an unbounded parameter `Name`).
            let l_acc = out.contains(&left.0);
            let r_acc = out.contains(&right.0);
            if (l_ctr && r_acc) || (r_ctr && l_acc) {
                out.insert(*id);
                changed = true;
            }
        }
    }
}

/// Close Add/Sub/Mul/Rem under IV loads + small consts when some exclusive bound is const.
fn mark_bounded_arith_tree(
    body: &Block,
    ivs: &HashSet<String>,
    bound: i64,
    all_defs: &HashMap<u32, Value>,
    nonneg_loads: &HashSet<u32>,
    out: &mut HashSet<u32>,
) {
    let mut seed: HashSet<u32> = out.clone();
    let seed_names: HashSet<String> = ivs.clone();
    let const_lim = bound
        .saturating_mul(bound)
        .min(NSW_BOUND_MAX.saturating_mul(NSW_BOUND_MAX));
    for (id, v) in all_defs {
        match v {
            Value::Int(n) if *n >= 0 && *n <= const_lim => {
                seed.insert(*id);
            }
            Value::Name(n) if ivs.contains(n) => {
                seed.insert(*id);
            }
            _ => {}
        }
    }
    for_each_block_dfs(body, &mut |b| {
        mark_name_loads_multi(b, ivs, &mut seed);
    });
    mark_small_factor_muls(all_defs, nonneg_loads, &mut seed);
    for id in &seed {
        if all_defs
            .get(id)
            .is_some_and(|v| matches!(v, Value::Binary { .. }))
        {
            out.insert(*id);
        }
    }

    let allow_acc = bound <= NSW_ACC_BOUND_MAX;
    let in_seed = |id: u32, seed: &HashSet<u32>, names: &HashSet<String>| {
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
            // Acc ± only when the Name side is already an IV / seed name — never
            // pull arbitrary mut slots into the NSW seed.
            let acc_add = allow_acc
                && matches!(op, BinOp::Add | BinOp::Sub)
                && ((l
                    && name_of(*right, all_defs).is_some_and(|n| seed_names.contains(&n)))
                    || (r
                        && name_of(*left, all_defs)
                            .is_some_and(|n| seed_names.contains(&n))));
            let rem_ok =
                matches!(op, BinOp::Rem) && l && const_of(*right, all_defs).is_some_and(|c| c > 1);
            if both || acc_add || rem_ok {
                seed.insert(*id);
                out.insert(*id);
                changed = true;
            }
        }
    }
}

fn mark_name_loads_multi(block: &Block, names: &HashSet<String>, out: &mut HashSet<u32>) {
    for op in &block.ops {
        if let Op::Let {
            local,
            value: Value::Name(n),
            ..
        } = op
        {
            if names.contains(n) {
                out.insert(local.0);
            }
        }
    }
}

pub(crate) fn collect_safe_divisor_locals(body: &Block) -> HashSet<u32> {
    let all_defs = core_collect_leaf_defs(body, false);
    let ge2_slots = collect_ge2_unit_slots(body, &all_defs);
    let mut out = HashSet::default();
    for (id, value) in &all_defs {
        match value {
            Value::Int(n) if *n != 0 && *n != -1 => {
                out.insert(*id);
            }
            Value::Name(n) if ge2_slots.contains(n) => {
                out.insert(*id);
            }
            _ => {}
        }
    }
    out
}

/// Locals that are `Name(iv)` loads inside a loop whose header proves `iv >= 0`
/// (strict `iv > k` with `k >= -1`, or `iv >= k` with `k >= 0`).
pub(crate) fn collect_nonneg_iv_load_locals(body: &Block) -> HashSet<u32> {
    let all_defs = core_collect_leaf_defs(body, false);
    let mut out = HashSet::default();
    for_each_block_dfs(body, &mut |b| {
        for op in &b.ops {
            if let Op::Let {
                value:
                    Value::Loop {
                        header,
                        body,
                        latch,
                    },
                ..
            } = op
            {
                for iv in nonneg_iv_names(header, &all_defs) {
                    mark_name_loads(body, iv.as_str(), &mut out);
                    mark_name_loads(latch, iv.as_str(), &mut out);
                }
            }
        }
    });
    out
}

fn nonneg_iv_names(header: &Block, all_defs: &HashMap<u32, Value>) -> HashSet<String> {
    let mut names = HashSet::default();
    // `iv > k` / `k < iv` with k ≥ -1 ⇒ iv ≥ 0.
    if let Some((iv, k)) = header_gt_const(header, all_defs) {
        if k >= -1 {
            names.insert(iv);
        }
    }
    // `iv >= k` (IV on the left) with k ≥ 0.
    if let Some((iv, k)) = header_ge_const(header, all_defs) {
        if k >= 0 {
            names.insert(iv);
        }
    }
    names
}

fn mark_name_loads(block: &Block, iv: &str, out: &mut HashSet<u32>) {
    for_each_block_dfs(block, &mut |b| {
        for op in &b.ops {
            if let Op::Let {
                local,
                value: Value::Name(n),
                ..
            } = op
            {
                if n == iv {
                    out.insert(local.0);
                }
            }
        }
    });
}

/// Mutable slots whose every assignment is `≥ 2` or `slot = slot + 1`.
fn collect_ge2_unit_slots(body: &Block, all_defs: &HashMap<u32, Value>) -> HashSet<String> {
    let mut assigns = HashMap::default();
    collect_assigns(body, &mut assigns);

    let mut ge2 = HashSet::default();
    for (name, vals) in assigns {
        let mut has_ge2_const = false;
        let mut ok = !vals.is_empty();
        for v in vals {
            match all_defs.get(&v.0) {
                Some(Value::Int(n)) if *n >= 2 => has_ge2_const = true,
                _ if is_unit_inc(v.0, name.as_str(), all_defs) => {}
                _ => {
                    ok = false;
                    break;
                }
            }
        }
        if ok && has_ge2_const {
            ge2.insert(name);
        }
    }
    ge2
}

#[cfg(test)]
#[path = "nsw_iv_tests.rs"]
mod tests;
