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

use lumia_core::{for_each_block_dfs, Block, Local, Op, Value};
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
    let leaf_defs = collect_leaf_defs(body);
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
    let l_name = name_of_local(*left, all_defs);
    let r_name = name_of_local(*right, all_defs);
    let l_c = const_i64(*left, all_defs);
    let r_c = const_i64(*right, all_defs);
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
    let res = header.result?;
    let Value::Binary {
        op, left, right, ..
    } = all_defs.get(&res.0)?
    else {
        return None;
    };
    if !matches!(op, BinOp::Le | BinOp::Lt) {
        return None;
    }
    let c = const_i64(*right, all_defs)
        .or_else(|| name_of_local(*right, all_defs).and_then(|n| iv_upper.get(&n).copied()))?;
    let Value::Binary {
        op: BinOp::Mul,
        left: a,
        right: b,
        ..
    } = all_defs.get(&left.0)?
    else {
        return None;
    };
    let na = name_of_local(*a, all_defs)?;
    let nb = name_of_local(*b, all_defs)?;
    if na == nb {
        Some((na, c))
    } else {
        None
    }
}

fn mark_square_mul(
    body: &Block,
    latch: &Block,
    iv: &str,
    all_defs: &HashMap<u32, Value>,
    out: &mut HashSet<u32>,
) {
    for block in [body, latch] {
        for op in &block.ops {
            if let Op::Let {
                local,
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
                let la = name_of_local(*left, all_defs);
                let ra = name_of_local(*right, all_defs);
                if la.as_deref() == Some(iv) && ra.as_deref() == Some(iv) {
                    out.insert(local.0);
                }
            }
        }
    }
    // Also scan all_defs (mul may be in header block).
    for (id, v) in all_defs {
        if let Value::Binary {
            op: BinOp::Mul,
            left,
            right,
            ..
        } = v
        {
            let la = name_of_local(*left, all_defs);
            let ra = name_of_local(*right, all_defs);
            if la.as_deref() == Some(iv) && ra.as_deref() == Some(iv) {
                out.insert(*id);
            }
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
            if is_unit_step_of(*dest, name, all_defs) {
                out.insert(*dest);
            }
        }
    });
}

fn is_unit_step_of(dest: u32, iv: &str, all_defs: &HashMap<u32, Value>) -> bool {
    let Some(Value::Binary {
        op, left, right, ..
    }) = all_defs.get(&dest)
    else {
        return false;
    };
    let step = match op {
        BinOp::Add => 1i64,
        BinOp::Sub => -1i64,
        _ => return false,
    };
    let l_iv = name_of_local(*left, all_defs).as_deref() == Some(iv);
    let r_iv = name_of_local(*right, all_defs).as_deref() == Some(iv);
    let l_c = const_i64(*left, all_defs);
    let r_c = const_i64(*right, all_defs);
    match step {
        1 => (l_iv && r_c == Some(1)) || (r_iv && l_c == Some(1)),
        -1 => l_iv && r_c == Some(1),
        _ => false,
    }
}

fn mark_small_factor_muls(
    all_defs: &HashMap<u32, Value>,
    nonneg_loads: &HashSet<u32>,
    out: &mut HashSet<u32>,
) {
    for (id, v) in all_defs {
        let Value::Binary {
            op: BinOp::Mul,
            left,
            right,
            ..
        } = v
        else {
            continue;
        };
        let (lc, rc) = (const_i64(*left, all_defs), const_i64(*right, all_defs));
        let ok = matches!(
            (lc, rc),
            (Some(c), _) if (1..=NSW_SMALL_FACTOR).contains(&c) && nonneg_loads.contains(&right.0)
        ) || matches!(
            (lc, rc),
            (_, Some(c)) if (1..=NSW_SMALL_FACTOR).contains(&c) && nonneg_loads.contains(&left.0)
        );
        if ok {
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
                && const_i64(*right, all_defs).is_some_and(|c| (0..=NSW_SMALL_FACTOR).contains(&c));
            let r_ok = out.contains(&right.0)
                && const_i64(*left, all_defs).is_some_and(|c| (0..=NSW_SMALL_FACTOR).contains(&c));
            if l_ok || r_ok {
                out.insert(*id);
                changed = true;
            }
        }
    }
}

/// Slots that are only ever assigned `0` or `self + 1` (Collatz `steps`, etc.).
fn collect_unit_counter_slots(body: &Block, all_defs: &HashMap<u32, Value>) -> HashSet<String> {
    let mut assigns: HashMap<String, Vec<u32>> = HashMap::default();
    for_each_block_dfs(body, &mut |b| {
        for op in &b.ops {
            if let Op::Assign {
                name,
                value: Local(v),
            } = op
            {
                assigns.entry(name.clone()).or_default().push(*v);
            }
        }
    });
    let mut out = HashSet::default();
    for (name, vals) in assigns {
        let mut has_zero = false;
        let mut ok = !vals.is_empty();
        for v in vals {
            match all_defs.get(&v) {
                Some(Value::Int(0)) => has_zero = true,
                Some(Value::Binary {
                    op: BinOp::Add,
                    left,
                    right,
                    ..
                }) => {
                    let l_self = name_of_local(*left, all_defs).as_deref() == Some(name.as_str());
                    let r_self = name_of_local(*right, all_defs).as_deref() == Some(name.as_str());
                    let l_one = const_i64(*left, all_defs) == Some(1);
                    let r_one = const_i64(*right, all_defs) == Some(1);
                    if !((l_self && r_one) || (r_self && l_one)) {
                        ok = false;
                        break;
                    }
                }
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
            let c = const_i64(*right, all_defs);
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

fn is_rem(l: Local, defs: &HashMap<u32, Value>) -> bool {
    matches!(defs.get(&l.0), Some(Value::Binary { op: BinOp::Rem, .. }))
}

/// Mark `Name(acc) + Name(unit_counter)` (and the reverse) as NSW.
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
            let ln = name_of_local(*left, all_defs);
            let rn = name_of_local(*right, all_defs);
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
        seed.contains(&id) || name_of_local(Local(id), all_defs).is_some_and(|n| names.contains(&n))
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
                    && name_of_local(*right, all_defs).is_some_and(|n| seed_names.contains(&n)))
                    || (r
                        && name_of_local(*left, all_defs)
                            .is_some_and(|n| seed_names.contains(&n))));
            let rem_ok =
                matches!(op, BinOp::Rem) && l && const_i64(*right, all_defs).is_some_and(|c| c > 1);
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

fn name_of_local(l: Local, defs: &HashMap<u32, Value>) -> Option<String> {
    match defs.get(&l.0)? {
        Value::Name(n) => Some(n.clone()),
        _ => None,
    }
}

fn const_i64(l: Local, defs: &HashMap<u32, Value>) -> Option<i64> {
    match defs.get(&l.0)? {
        Value::Int(n) => Some(*n),
        _ => None,
    }
}

pub(crate) fn collect_leaf_defs(body: &Block) -> HashMap<u32, Value> {
    let mut all_defs: HashMap<u32, Value> = HashMap::default();
    for_each_block_dfs(body, &mut |b| {
        for op in &b.ops {
            if let Op::Let { local, value, .. } = op {
                if matches!(
                    value,
                    Value::Int(_)
                        | Value::Float(_)
                        | Value::Name(_)
                        | Value::Binary { .. }
                        | Value::Builtin { .. }
                ) {
                    all_defs.insert(local.0, value.clone());
                }
            }
        }
    });
    all_defs
}

/// Locals safe as signed `div`/`rem` RHS: `Int` ∉ {0,-1}, or loads of slots that
/// are only ever assigned `≥ 2` or `self + 1` (so never 0 / -1).
pub(crate) fn collect_safe_divisor_locals(body: &Block) -> HashSet<u32> {
    let all_defs = collect_leaf_defs(body);
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
    let all_defs = collect_leaf_defs(body);
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
    let Some(res) = header.result else {
        return names;
    };
    let Some(Value::Binary {
        op, left, right, ..
    }) = all_defs.get(&res.0)
    else {
        return names;
    };
    // Only `Name(iv) ▷ const` (IV on the left).
    let Some(iv) = name_of_local(*left, all_defs) else {
        return names;
    };
    let Some(k) = const_i64(*right, all_defs) else {
        return names;
    };
    let ok = match op {
        BinOp::Gt => k >= -1, // iv >= k+1 >= 0
        BinOp::Ge => k >= 0,  // iv >= k >= 0
        _ => false,
    };
    if ok {
        names.insert(iv);
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
    let mut assigns: HashMap<String, Vec<u32>> = HashMap::default();
    for_each_block_dfs(body, &mut |b| {
        for op in &b.ops {
            if let Op::Assign {
                name,
                value: Local(v),
            } = op
            {
                assigns.entry(name.clone()).or_default().push(*v);
            }
        }
    });

    let mut ge2 = HashSet::default();
    for (name, vals) in assigns {
        let mut has_ge2_const = false;
        let mut ok = !vals.is_empty();
        for v in vals {
            match all_defs.get(&v) {
                Some(Value::Int(n)) if *n >= 2 => has_ge2_const = true,
                Some(Value::Binary {
                    op: BinOp::Add,
                    left,
                    right,
                    ..
                }) => {
                    let l_self = name_of_local(*left, all_defs).as_deref() == Some(name.as_str());
                    let r_self = name_of_local(*right, all_defs).as_deref() == Some(name.as_str());
                    let l_one = const_i64(*left, all_defs) == Some(1);
                    let r_one = const_i64(*right, all_defs) == Some(1);
                    if !((l_self && r_one) || (r_self && l_one)) {
                        ok = false;
                        break;
                    }
                }
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
mod tests {
    use super::*;
    use lumia_core::compile_source_to_core;

    #[test]
    fn marks_lt_unit_increment() {
        let core = compile_source_to_core(
            r#"
module M
val main = {
  var i = 0
  var s = 0
  for i < 10 {
    s = s + i
    i = i + 1
  }
  s
}
"#,
        )
        .unwrap();
        let main = core.functions.iter().find(|f| f.name == "main").unwrap();
        let nsw = collect_nsw_binop_locals(&main.body);
        assert!(!nsw.is_empty(), "expected i=i+1 under i<10 to be NSW-safe");
    }

    #[test]
    fn marks_shared_const_one_outside_loop() {
        // Mimic bench lowering: `1` defined once, reused inside the loop.
        let core = compile_source_to_core(
            r#"
module M
val main = {
  var i = 0
  val one = 1
  for i < 10 {
    i = i + one
  }
  i
}
"#,
        )
        .unwrap();
        let main = core.functions.iter().find(|f| f.name == "main").unwrap();
        let nsw = collect_nsw_binop_locals(&main.body);
        assert!(
            !nsw.is_empty(),
            "i=i+one under i<10 should be NSW-safe even if `one` is outer"
        );
    }

    #[test]
    fn marks_le_const_increment() {
        let core = compile_source_to_core(
            r#"
module M
val main = {
  var i = 0
  var s = 0
  for i <= 10 {
    s = s + i
    i = i + 1
  }
  s
}
"#,
        )
        .unwrap();
        let main = core.functions.iter().find(|f| f.name == "main").unwrap();
        let nsw = collect_nsw_binop_locals(&main.body);
        assert!(!nsw.is_empty(), "i=i+1 under i<=10 (const) is NSW-safe");
    }

    #[test]
    fn skips_le_unknown_bound() {
        let core = compile_source_to_core(
            r#"
module M
val main(limit) = {
  var i = 0
  for i <= limit {
    i = i + 1
  }
  i
}
"#,
        )
        .unwrap();
        let main = core.functions.iter().find(|f| f.name == "main").unwrap();
        let nsw = collect_nsw_binop_locals(&main.body);
        assert!(
            nsw.is_empty(),
            "i=i+1 under i<=limit (unknown) must keep overflow checks"
        );
    }

    #[test]
    fn marks_matmul_iv_increments() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/bench_cpu.lm");
        let src = std::fs::read_to_string(&path).unwrap();
        let core =
            lumia_opt::compile_source_to_optimized(&src, &lumia_opt::OptOptions::for_build(true))
                .unwrap();
        // Prefer the const-specialized clone (`matmulChecksum$c_160`) when present.
        let f = core
            .functions
            .iter()
            .find(|f| f.name.starts_with("matmulChecksum$c_"))
            .or_else(|| core.functions.iter().find(|f| f.name == "matmulChecksum"))
            .unwrap();
        let nsw = collect_nsw_binop_locals(&f.body);
        assert!(
            nsw.len() >= 3,
            "expected i/j/k unit steps under strict <, got {nsw:?}"
        );
    }

    #[test]
    fn marks_const_bound_mul_tree() {
        let core = compile_source_to_core(
            r#"
module M
val main = {
  var i = 0
  var s = 0
  for i < 10 {
    s = s + i * 10 + 1
    i = i + 1
  }
  s
}
"#,
        )
        .unwrap();
        let main = core.functions.iter().find(|f| f.name == "main").unwrap();
        let nsw = collect_nsw_binop_locals(&main.body);
        assert!(
            nsw.len() >= 2,
            "expected unit step + i*10/+ tree under i<10, got {nsw:?}"
        );
    }

    #[test]
    fn marks_is_prime_d_as_safe_divisor() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/bench_cpu.lm");
        let src = std::fs::read_to_string(&path).unwrap();
        let core =
            lumia_opt::compile_source_to_optimized(&src, &lumia_opt::OptOptions::for_build(true))
                .unwrap();
        let f = core
            .functions
            .iter()
            .find(|f| f.name == "countPrimes")
            .unwrap();
        let safe = collect_safe_divisor_locals(&f.body);
        assert!(
            !safe.is_empty(),
            "inlined isPrime `d` (init 2, +=1) should yield safe divisor locals"
        );
    }

    #[test]
    fn rejects_zero_init_slot_as_divisor() {
        let core = compile_source_to_core(
            r#"
module M
val main = {
  var i = 0
  var s = 0
  for i < 10 {
    s = s + (s % i)
    i = i + 1
  }
  s
}
"#,
        )
        .unwrap();
        let main = core.functions.iter().find(|f| f.name == "main").unwrap();
        // `i` starts at 0 — must not treat Name(i) loads as safe divisors.
        // Int constants like the loop bound may still be marked; check no Name-based
        // safety by ensuring ge2 path does not fire for i: only Int≠{0,-1} locals.
        let safe = collect_safe_divisor_locals(&main.body);
        let ge2 = collect_ge2_unit_slots(&main.body, &collect_leaf_defs(&main.body));
        assert!(
            !ge2.contains("i"),
            "i starts at 0, got ge2={ge2:?} safe={safe:?}"
        );
    }

    #[test]
    fn marks_collatz_x_loads_nonneg() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/bench_cpu.lm");
        let src = std::fs::read_to_string(&path).unwrap();
        let core =
            lumia_opt::compile_source_to_optimized(&src, &lumia_opt::OptOptions::for_build(true))
                .unwrap();
        let f = core
            .functions
            .iter()
            .find(|f| f.name == "collatzTotal")
            .unwrap();
        let nonneg = collect_nonneg_iv_load_locals(&f.body);
        assert!(
            !nonneg.is_empty(),
            "inlined collatzSteps `x` under x>1 should be nonneg loads"
        );
    }

    #[test]
    fn fib_match01_subs_stay_checked() {
        let core = compile_source_to_core(
            r#"
module M
val fib(n) = {
  n match {
    0 -> 0
    1 -> 1
    _ -> fib(n - 1) + fib(n - 2)
  }
}
val main = fib(10)
"#,
        )
        .unwrap();
        let fib = core.functions.iter().find(|f| f.name == "fib").unwrap();
        let nsw = collect_nsw_binop_locals(&fib.body);
        // Residual arm includes n=i64::MIN where n-1 overflows — keep checked.
        let defs = collect_leaf_defs(&fib.body);
        let sub_locals: Vec<_> = defs
            .iter()
            .filter_map(|(id, v)| match v {
                Value::Binary { op: BinOp::Sub, .. } => Some(*id),
                _ => None,
            })
            .collect();
        assert!(
            sub_locals.iter().all(|id| !nsw.contains(id)),
            "fib n-1/n-2 must not be NSW: {sub_locals:?} nsw={nsw:?}"
        );
        let add_locals: Vec<_> = defs
            .iter()
            .filter_map(|(id, v)| match v {
                Value::Binary { op: BinOp::Add, .. } => Some(*id),
                _ => None,
            })
            .collect();
        assert!(
            add_locals.iter().all(|id| !nsw.contains(id)),
            "fib add must not be NSW: {add_locals:?} nsw={nsw:?}"
        );
    }
}
