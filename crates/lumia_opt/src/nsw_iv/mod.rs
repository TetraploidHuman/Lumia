//! Mark loop induction updates — and const-bounded IV arith trees — as NSW-safe.
//!
//! Gated by Cargo feature `nsw-iv` (default on). Without it, [`NswIvPass`] clears
//! NSW / divisor / nonneg sets on each [`CoreFun`].
//!
//! Codegen reads the sidecar sets on [`lumia_core::CoreFun`] and still builds
//! `leaf_defs` locally for COW / domain-SR peeps.
//!
//! ## Unit steps
//! - Strict `<`/`>`: `iv = iv ± 1` cannot overflow signed i64.
//! - Inclusive `<=`/`>=`: same when the bound is a **known constant**, or a
//!   **named** upper already in `iv_upper` with `U < MAX` (nested `i <= n`
//!   under `n < K`). Unknown `n` in `i <= n` may be `i64::MAX`.
//!
//! ## Bounded trees
//! With a small const bound (`1..=NSW_BOUND_MAX`), Add/Sub/Mul(/Rem) closed under
//! Accumulators: `Name(acc) + nsw` is included when the bound is ≤ `NSW_ACC_BOUND_MAX`
//! (tree-acc bootstrap: init 0, assigns only `self + NSW` — matmul `cell +=`).
//! With a larger const IV upper (`≤ NSW_IV_UPPER_MAX`), `acc += unit_counter` is
//! also NSW when the addend slot is only ever `0` / `+= 1` (Collatz `total += steps`).
//!
//! ## Extra peeps
//! - `c * nonneg_iv` for tiny `|c|` (Collatz `3*x`)
//! - `iv * iv` under `iv*iv ≤ C` (or ≤ outer const-bounded IV) headers
//! - nonnegative Int literal `+`/`*` whose result fits i64 (checked at mark time)
//! - const-bounded nonneg IV `+`/`*` nonnegative literal when `U.checked_*` fits
//! - two const-bounded nonneg IVs `+`/`*` when `U1.checked_{add,mul}(U2)` fits (`i+j` / `i*j`)
//! - open exclusive `i < n`: worst-case `U = MAX-1` so `i+1` is NSW (not `i+50` / `i*3`);
//!   if `n` already has an `iv_upper`, use that instead (nested loops)
//! - open inclusive `i <= n`: seed / unit-step only when `n` has a known `iv_upper`
//! - safe `Div` / small-const `Rem` always (acc+div/rem still needs const IV bounds)
//! - Fib-style `match { 0|1 → …; _ → …(n-1)/(n-2) }`: mark `n-1`/`n-2` NSW
//!   in the residual arm (add of recursive results stays checked — fib(93)+ overflows)
//!
//! Codegen may also set LLVM **`nuw`** on NSW `Add`/`Mul` when both operands are
//! proven nonnegative (signed no-wrap ⇒ unsigned no-wrap for `a,b ≥ 0`).

#[cfg(feature = "nsw-iv")]
mod bounds;
#[cfg(feature = "nsw-iv")]
mod divisors;
#[cfg(feature = "nsw-iv")]
mod marks;

use lumia_core::CoreModule;
use rustc_hash::FxHashSet as HashSet;

#[cfg(feature = "nsw-iv")]
use bounds::{iv_bound_info, mark_square_mul, square_bound};
#[cfg(feature = "nsw-iv")]
use lumia_core::{
    collect_leaf_defs as core_collect_leaf_defs, for_each_loop_in_block, Block, Value,
};
#[cfg(feature = "nsw-iv")]
use lumia_syntax::Sym;
#[cfg(feature = "nsw-iv")]
use marks::{
    mark_acc_plus_bounded_rem, mark_acc_plus_safe_div, mark_acc_plus_unit_counter,
    mark_bounded_arith_tree, mark_bounded_nonneg_pair_add, mark_bounded_rem_bins,
    mark_nonneg_const_add, mark_nonneg_const_mul, mark_nonneg_iv_add_mul_bounded, mark_nonneg_sub,
    mark_safe_div_bins, mark_small_factor_muls, mark_unit_steps,
};

#[cfg(all(test, feature = "nsw-iv"))]
pub(crate) use divisors::collect_ge1_unit_slots;
#[cfg(all(test, feature = "nsw-iv"))]
pub(crate) use divisors::collect_ge2_unit_slots;
#[cfg(feature = "nsw-iv")]
pub(crate) use divisors::{collect_nonneg_iv_load_locals, collect_safe_divisor_locals};

/// Max loop bound for IV×bound arith trees (products must fit i64 with margin).
#[cfg(feature = "nsw-iv")]
const NSW_BOUND_MAX: i64 = 30_000;
/// Cap for recording IV uppers used only to prove `d*d ≤ n` NSW.
#[cfg(feature = "nsw-iv")]
const NSW_IV_UPPER_MAX: i64 = 1_000_000;
/// Outer-loop const bound gate for unit-counter acc peeps (`total += steps`).
/// Larger than [`NSW_IV_UPPER_MAX`] so Collatz 2.5M/3M still qualify without
/// widening square-bound / bounded-tree caps.
#[cfg(feature = "nsw-iv")]
const NSW_UNIT_ACC_LOOP_MAX: i64 = 10_000_000;
/// Tighter cap so `acc += O(B^4)` over `O(B^3)` iters still fits i64 (matmul-style).
#[cfg(feature = "nsw-iv")]
const NSW_ACC_BOUND_MAX: i64 = 2_000;
/// `|c|` for `c * nonneg_iv` NSW peep (Collatz `3*x`).
#[cfg(feature = "nsw-iv")]
const NSW_SMALL_FACTOR: i64 = 16;
/// `acc += (… % C)` with `C ≤ this` is NSW under const-bounded IVs (`≤ NSW_BOUND_MAX`):
/// rem ∈ `[0,C)` so `B²·C` fits i64 for `B ≤ 30_000`, `C ≤ 1_000_000`.
#[cfg(feature = "nsw-iv")]
const NSW_REM_MOD_MAX: i64 = 1_000_000;

/// Locals that are `Binary` Add/Sub/Mul(/Rem) results proven safe for NSW emission.
#[cfg(all(test, feature = "nsw-iv"))]
pub(crate) fn collect_nsw_binop_locals(body: &Block) -> HashSet<u32> {
    let leaf_defs = core_collect_leaf_defs(body, false);
    let nonneg = collect_nonneg_iv_load_locals(body);
    collect_nsw_binop_locals_inner(body, &leaf_defs, &nonneg)
}

/// Function-wide NSW / IV peep facts (no `leaf_defs` — that stays emit-local).
#[derive(Debug, Default, Clone)]
pub(crate) struct NswFacts {
    pub nsw_binop_locals: HashSet<u32>,
    pub safe_divisor_locals: HashSet<u32>,
    pub nonneg_iv_load_locals: HashSet<u32>,
}

/// Analyze `body` for NSW / divisor / nonneg IV facts.
#[cfg(feature = "nsw-iv")]
pub(crate) fn analyze_nsw(body: &Block) -> NswFacts {
    let leaf_defs = core_collect_leaf_defs(body, false);
    let nonneg_iv_load_locals = collect_nonneg_iv_load_locals(body);
    let safe_divisor_locals = collect_safe_divisor_locals(body);
    let nsw_binop_locals = collect_nsw_binop_locals_inner(body, &leaf_defs, &nonneg_iv_load_locals);
    NswFacts {
        nsw_binop_locals,
        safe_divisor_locals,
        nonneg_iv_load_locals,
    }
}

#[cfg(not(feature = "nsw-iv"))]
pub(crate) fn analyze_nsw(_body: &lumia_core::Block) -> NswFacts {
    NswFacts::default()
}

/// Late opt pass: stamp NSW facts onto each [`lumia_core::CoreFun`] after SSA settles.
pub(crate) struct NswIvPass;

impl NswIvPass {
    pub(crate) fn run(self, module: &mut CoreModule) {
        for fun in &mut module.functions {
            if fun.external.is_some() {
                fun.nsw_binop_locals = HashSet::default();
                fun.safe_divisor_locals = HashSet::default();
                fun.nonneg_iv_load_locals = HashSet::default();
                continue;
            }
            let facts = analyze_nsw(&fun.body);
            fun.nsw_binop_locals = facts.nsw_binop_locals;
            fun.safe_divisor_locals = facts.safe_divisor_locals;
            fun.nonneg_iv_load_locals = facts.nonneg_iv_load_locals;
        }
    }
}

#[cfg(feature = "nsw-iv")]
fn collect_nsw_binop_locals_inner(
    body: &Block,
    all_defs: &rustc_hash::FxHashMap<u32, Value>,
    nonneg_loads: &HashSet<u32>,
) -> HashSet<u32> {
    let mut out = HashSet::default();
    let mut bounded_ivs: HashSet<Sym> = HashSet::default();
    let mut iv_upper: rustc_hash::FxHashMap<Sym, i64> = rustc_hash::FxHashMap::default();
    let mut max_bound: i64 = 0;
    let mut max_loop_const: i64 = 0;

    // Pass 1: unit steps + collect const-bounded IV uppers.
    for_each_loop_in_block(body, &mut |header, body, latch| {
        let info = iv_bound_info(header, all_defs);
        // Copy before mutating `iv_upper` for this loop's IVs.
        let named_upper = info
            .bound_name
            .as_ref()
            .and_then(|n| iv_upper.get(n).copied());
        // Inclusive `i <= n`: unit ±1 is NSW when `n` itself has a known
        // upper `U < MAX` (nested `n < K` / `n < limit` with U=MAX-1).
        let unit_ok = info.strict
            || info
                .bound_const
                .is_some_and(|b| (0..=i64::MAX - 2).contains(&b))
            || (!info.strict
                && info.is_upper
                && named_upper.is_some_and(|u| (0..=i64::MAX - 1).contains(&u)));
        if unit_ok {
            for name in &info.ivs {
                mark_unit_steps(body, name, all_defs, &mut out);
                mark_unit_steps(latch, name, all_defs, &mut out);
            }
            // Do **not** NSW-mark arbitrary `x = x ± 1` in the body: a separate
            // counter can still overflow while the IV stays in range.
        }
        if let Some(b) = info.bound_const {
            if b > 0 {
                max_loop_const = max_loop_const.max(b);
            }
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
        } else if info.is_upper {
            if let Some(u) = named_upper {
                // Named upper already recorded (outer IV / prior loop):
                // exclusive `i < n` and inclusive `i <= n` both have
                // max i ≤ U (exclusive is tighter; storing U matches
                // const `i < K` which records K).
                for name in &info.ivs {
                    let e = iv_upper.entry(name.clone()).or_insert(u);
                    *e = (*e).max(u);
                }
            } else if info.strict {
                // Open exclusive `i < n`: worst-case max i is `i64::MAX - 1`
                // (when n = MAX). Seeds `i + 1` via mark_nonneg_iv_add_mul_bounded
                // without unlocking bounded-tree / square peeps (caps below).
                for name in &info.ivs {
                    let e = iv_upper.entry(name.clone()).or_insert(i64::MAX - 1);
                    *e = (*e).max(i64::MAX - 1);
                }
            }
        }
    });

    // Pass 2: `d*d ≤ C` / `d*d ≤ bounded_iv` — mark square + `d = d+1`.
    for_each_loop_in_block(body, &mut |header, body, latch| {
        if let Some((iv, c)) = square_bound(header, all_defs, &iv_upper) {
            if (1..=NSW_IV_UPPER_MAX).contains(&c) {
                mark_unit_steps(body, &iv, all_defs, &mut out);
                mark_unit_steps(latch, &iv, all_defs, &mut out);
                mark_square_mul(body, latch, &iv, all_defs, &mut out);
                let d_max = ((c as f64).sqrt().floor() as i64) + 2;
                let e = iv_upper.entry(iv.clone()).or_insert(d_max);
                *e = (*e).max(d_max);
                if (1..=NSW_BOUND_MAX).contains(&c) {
                    bounded_ivs.insert(iv);
                    max_bound = max_bound.max(d_max);
                }
            }
        }
    });

    if !bounded_ivs.is_empty() {
        mark_bounded_arith_tree(
            body,
            &bounded_ivs,
            max_bound,
            all_defs,
            nonneg_loads,
            &mut out,
        );
        // Even when bound > NSW_ACC_BOUND_MAX, `s += (e % C)` / `s += (N/i)` are
        // safe under rem-mod / safe-divisor constraints.
        mark_acc_plus_bounded_rem(body, all_defs, &mut out);
        mark_acc_plus_safe_div(body, all_defs, &mut out);
    }
    mark_small_factor_muls(all_defs, nonneg_loads, &iv_upper, &mut out);
    mark_nonneg_sub(all_defs, nonneg_loads, &mut out);
    mark_nonneg_const_add(all_defs, &mut out);
    mark_nonneg_const_mul(all_defs, &mut out);
    // Const-upper IV + nonnegative literal (e.g. `i + 2` / `i * 3` under `i < N`).
    // Also open exclusive `i < n` via worst-case upper `MAX-1` (allows `i+1` only),
    // and open inclusive `i <= n` when `n` already has an `iv_upper`.
    mark_nonneg_iv_add_mul_bounded(all_defs, nonneg_loads, &iv_upper, &mut out);
    mark_bounded_nonneg_pair_add(all_defs, nonneg_loads, &iv_upper, &mut out);

    // Safe Div / small-const Rem are NSW on their own (no IV bound needed).
    mark_safe_div_bins(body, all_defs, &mut out);
    mark_bounded_rem_bins(all_defs, &mut out);

    // `total += steps` under a large const-bounded outer IV: RHS is a unit
    // counter (init 0, only +=1). Collatz max steps per n is tiny vs i64.
    // Gate on raw loop consts (not `iv_upper`) so 2.5M/3M Collatz still qualify;
    // do **not** use open-upper `MAX-1` seeds here (quadratic risk).
    if (1..=NSW_UNIT_ACC_LOOP_MAX).contains(&max_loop_const) {
        mark_acc_plus_unit_counter(body, all_defs, &mut out);
    }

    // Fib-style `n-1`/`n-2` after match 0|1 is **not** NSW: residual includes
    // `n = i64::MIN` where `n-1` overflows. Keep checked sub there.
    out
}

#[cfg(all(test, feature = "nsw-iv"))]
#[path = "../nsw_iv_tests.rs"]
mod tests;
