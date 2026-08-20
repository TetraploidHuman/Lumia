use lumia_core::CoreBinOp as BinOp;
use lumia_core::{const_of, is_name_mul_name, name_of, Block, Local, Value};
use lumia_syntax::Sym;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

pub(super) struct IvBoundInfo {
    pub ivs: HashSet<Sym>,
    pub bound_const: Option<i64>,
    /// True for `<` / `>` (unit ±1 always NSW-safe).
    pub strict: bool,
    /// True when the compare is an **upper** bound on the IV (`i < n` / `n > i`),
    /// not a lower bound (`i > K`). Open exclusive uppers may seed a worst-case
    /// `iv_upper = i64::MAX - 1` for `i + 1`-class peeps.
    pub is_upper: bool,
    /// Non-const upper bound slot (`i < n` / `i <= n` / `n > i` / `n >= i`).
    pub bound_name: Option<Sym>,
}

/// IV names + optional **upper** const bound from `<`/`>`/`<=`/`>=`.
///
/// Only the induction-side `Name` is recorded (never the bound variable).
/// `bound_const` is set only when the constant is an exclusive/inclusive **upper**
/// bound (`i < K` / `K > i`), not a lower bound (`i > K`), so mul/acc trees stay sound.
pub(super) fn iv_bound_info(header: &Block, all_defs: &HashMap<u32, Value>) -> IvBoundInfo {
    let empty = IvBoundInfo {
        ivs: HashSet::default(),
        bound_const: None,
        strict: false,
        is_upper: false,
        bound_name: None,
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
    let (iv, bound_const, is_upper, bound_name) = match op {
        // `iv < K` / `iv <= K` — K is an upper bound.
        BinOp::Lt | BinOp::Le if r_c.is_some() && l_name.is_some() => (l_name, r_c, true, None),
        // `K > iv` / `K >= iv` — K is an upper bound on iv.
        BinOp::Gt | BinOp::Ge if l_c.is_some() && r_name.is_some() => (r_name, l_c, true, None),
        // `iv < n` / `iv <= n` / `n > iv` / `n >= iv` — named upper, no const.
        BinOp::Lt | BinOp::Le if l_name.is_some() => (l_name, None, true, r_name),
        BinOp::Gt | BinOp::Ge if r_name.is_some() => (r_name, None, true, l_name),
        // Lower-bound forms (`iv > K`) — unit ±1 on iv is still NSW under strict
        // compares, but K must not seed bounded arith trees / open-upper peeps.
        BinOp::Gt | BinOp::Ge if l_name.is_some() && r_c.is_some() => (l_name, None, false, None),
        BinOp::Lt | BinOp::Le if r_name.is_some() && l_c.is_some() => (r_name, None, false, None),
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
        is_upper,
        bound_name,
    }
}

/// `Name(iv) * Name(iv) ≤ Const` or `≤ Name(bounded)` (isPrime trial loop).
pub(super) fn square_bound(
    header: &Block,
    all_defs: &HashMap<u32, Value>,
    iv_upper: &HashMap<Sym, i64>,
) -> Option<(Sym, i64)> {
    let (iv, bound, _strict) = lumia_core::header_name_sq_cmp(header, all_defs)?;
    let c = const_of(bound, all_defs)
        .or_else(|| name_of(bound, all_defs).and_then(|n| iv_upper.get(&n).copied()))?;
    Some((iv, c))
}

pub(super) fn mark_square_mul(
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
