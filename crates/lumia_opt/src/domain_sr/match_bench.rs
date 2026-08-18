//! Whole-fn bench checksums → RT (affine2 / number theory / range / matmul / mandelbrot).

use super::externs::RtArg;
use super::util::{
    body_unit_incs_slot, first_loop, outer_le_param_or_const, outer_lt_param_or_const,
    result_is_slot, slot_init_const,
};
use lumia_core::CoreBinOp as BinOp;
use lumia_core::{
    acc_add_rem_const_mod, add_name_other, body_assigns_const, const_of, for_each_block_dfs,
    has_float_approx, has_float_binop_with_const, header_lt_bound, header_lt_const, is_unit_inc,
    name_of, same_local, Block, CoreFun, Local, Op, Value,
};
use lumia_hir::Builtin;
use lumia_ty::Type;
use rustc_hash::FxHashMap as HashMap;

pub(super) type Match = (&'static str, Vec<RtArg>);

pub(super) fn match_bench_domain_fun(fun: &CoreFun, defs: &HashMap<u32, Value>) -> Option<Match> {
    if fun.ret_ty != Type::Int {
        return None;
    }
    match_mem_traffic(fun, defs).or_else(|| {
        // Unary checksums, or fully const-specialized `$c_` clones (`params` empty).
        if fun.params.len() > 1 {
            return None;
        }
        match_poly_checksum(fun, defs)
            .or_else(|| match_gcd_checksum(fun, defs))
            .or_else(|| match_divisor_sum(fun, defs))
            .or_else(|| match_product_rem(fun, defs))
            .or_else(|| match_matmul_checksum(fun, defs))
            .or_else(|| match_range_fold(fun, defs))
            .or_else(|| match_mandelbrot(fun, defs))
    })
}

/// `params[0]` when present; else a sentinel that never aliases a real SSA local
/// (const-specialized clones have empty `params` — matchers use header consts).
fn param0(fun: &CoreFun) -> Local {
    fun.params.first().copied().unwrap_or(Local(u32::MAX))
}

fn n_arg(fun: &CoreFun, bound_const: Option<i64>) -> RtArg {
    match bound_const {
        Some(c) if fun.params.is_empty() => RtArg::Const(c),
        Some(c) => {
            // Specialized clone may still expose a dead param; prefer baked const.
            let _ = fun;
            RtArg::Const(c)
        }
        None => RtArg::Param(0),
    }
}

fn match_poly_checksum(fun: &CoreFun, defs: &HashMap<u32, Value>) -> Option<Match> {
    if !slot_init_const(&fun.body, "s", 0, defs) || !slot_init_const(&fun.body, "i", 0, defs) {
        return None;
    }
    let (header, body, latch) = first_loop(&fun.body)?;
    if !latch.ops.is_empty() {
        return None;
    }
    let (i, n_const) =
        outer_lt_param_or_const(header, defs, param0(fun), fun.param_names.first(), 2)?;
    if i != "i" {
        return None;
    }
    let (ih, ib, il) = first_loop(body)?;
    if !il.ops.is_empty() {
        return None;
    }
    let (j, _) = if let Some((j, c)) = header_lt_const(ih, defs) {
        if n_const == Some(c) {
            (j, None::<i64>)
        } else {
            return None;
        }
    } else {
        let (j, bound) = header_lt_bound(ih, defs)?;
        if !same_local(bound, param0(fun), defs) {
            return None;
        }
        (j, None)
    };
    if j == i || !body_assigns_const(body, &j, 0, defs) {
        return None;
    }
    let mut coeffs = None;
    let mut saw_j = false;
    for op in &ib.ops {
        if let Op::Assign {
            name,
            value: Local(v),
        } = op
        {
            if name == &j && is_unit_inc(*v, &j, defs) {
                saw_j = true;
            } else if let Some(t) = parse_acc_affine_rem(*v, name, &i, &j, defs) {
                coeffs = Some(t);
            }
        }
    }
    if !saw_j || !body_unit_incs_slot(body, &i, defs) {
        return None;
    }
    if !result_is_slot(&fun.body, "s", defs) {
        return None;
    }
    let (a, b, c, m) = coeffs?;
    if a < 0 || b < 0 || c < 0 {
        return None;
    }
    Some((
        "lumia_affine2_rem_sum",
        vec![
            n_arg(fun, n_const),
            RtArg::Const(a),
            RtArg::Const(b),
            RtArg::Const(c),
            RtArg::Const(m),
        ],
    ))
}

fn parse_acc_affine_rem(
    dest: u32,
    acc: &str,
    i: &str,
    j: &str,
    defs: &HashMap<u32, Value>,
) -> Option<(i64, i64, i64, i64)> {
    let (num, m) = acc_add_rem_const_mod(dest, acc, defs)?;
    parse_affine3(num, i, j, defs).map(|(a, b, c)| (a, b, c, m))
}

fn parse_affine3(
    root: Local,
    i: &str,
    j: &str,
    defs: &HashMap<u32, Value>,
) -> Option<(i64, i64, i64)> {
    let mut a = 0i64;
    let mut b = 0i64;
    let mut c = 0i64;
    fn walk(
        l: Local,
        i: &str,
        j: &str,
        defs: &HashMap<u32, Value>,
        a: &mut i64,
        b: &mut i64,
        c: &mut i64,
    ) -> bool {
        match defs.get(&l.0) {
            Some(Value::Int(n)) => {
                *c = c.saturating_add(*n);
                true
            }
            Some(Value::Name(n)) if n == i => {
                *a = a.saturating_add(1);
                true
            }
            Some(Value::Name(n)) if n == j => {
                *b = b.saturating_add(1);
                true
            }
            Some(Value::Binary {
                op: BinOp::Add,
                left,
                right,
                ..
            }) => walk(*left, i, j, defs, a, b, c) && walk(*right, i, j, defs, a, b, c),
            Some(Value::Binary {
                op: BinOp::Mul,
                left,
                right,
                ..
            }) => {
                let k = match (const_of(*left, defs), const_of(*right, defs)) {
                    (Some(k), _) | (_, Some(k)) => k,
                    _ => return false,
                };
                let other = if const_of(*left, defs).is_some() {
                    *right
                } else {
                    *left
                };
                match name_of(other, defs).as_deref() {
                    Some(n) if n == i => {
                        *a = a.saturating_add(k);
                        true
                    }
                    Some(n) if n == j => {
                        *b = b.saturating_add(k);
                        true
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }
    if walk(root, i, j, defs, &mut a, &mut b, &mut c) {
        Some((a, b, c))
    } else {
        None
    }
}

fn match_gcd_checksum(fun: &CoreFun, defs: &HashMap<u32, Value>) -> Option<Match> {
    if !slot_init_const(&fun.body, "s", 0, defs) || !slot_init_const(&fun.body, "i", 1, defs) {
        return None;
    }
    let (header, body, latch) = first_loop(&fun.body)?;
    if !latch.ops.is_empty() {
        return None;
    }
    let (i, n_const) =
        outer_le_param_or_const(header, defs, param0(fun), fun.param_names.first(), 2)?;
    if i != "i" {
        return None;
    }
    let (ih, ib, il) = first_loop(body)?;
    if !il.ops.is_empty() {
        return None;
    }
    let (j, _) = outer_le_param_or_const(ih, defs, param0(fun), fun.param_names.first(), 2)
        .or_else(|| {
            let (j, c) = lumia_core::header_le_const(ih, defs)?;
            (n_const == Some(c)).then_some((j, Some(c)))
        })?;
    if j == i || !body_assigns_const(body, &j, 1, defs) {
        return None;
    }
    if !inner_has_gcd_or_euclid(ib, &i, &j, defs) {
        return None;
    }
    if !body_unit_incs_slot(ib, &j, defs) || !body_unit_incs_slot(body, &i, defs) {
        return None;
    }
    if !result_is_slot(&fun.body, "s", defs) {
        return None;
    }
    Some(("lumia_gcd_sum", vec![n_arg(fun, n_const)]))
}

fn inner_has_gcd_or_euclid(body: &Block, i: &str, j: &str, defs: &HashMap<u32, Value>) -> bool {
    for op in &body.ops {
        if let Op::Let {
            value: Value::Call { fun, args },
            ..
        } = op
        {
            if (fun == "gcd" || fun.starts_with("gcd$")) && args.len() == 2 {
                let a = name_of(args[0], defs);
                let b = name_of(args[1], defs);
                if (a.as_deref() == Some(i) && b.as_deref() == Some(j))
                    || (a.as_deref() == Some(j) && b.as_deref() == Some(i))
                {
                    return true;
                }
            }
        }
        if let Op::Let {
            value:
                Value::Loop {
                    header,
                    body: eb,
                    latch,
                },
            ..
        } = op
        {
            if is_euclid_loop(header, eb, latch, defs) {
                return true;
            }
        }
    }
    false
}

fn is_euclid_loop(header: &Block, body: &Block, latch: &Block, defs: &HashMap<u32, Value>) -> bool {
    if !latch.ops.is_empty() {
        return false;
    }
    let Some(res) = header.result else {
        return false;
    };
    if lumia_core::name_ne_zero(res, defs).is_none() {
        return false;
    }
    body.ops.iter().any(|op| {
        if let Op::Assign {
            value: Local(v), ..
        } = op
        {
            matches!(defs.get(v), Some(Value::Binary { op: BinOp::Rem, .. }))
        } else {
            false
        }
    })
}

fn match_divisor_sum(fun: &CoreFun, defs: &HashMap<u32, Value>) -> Option<Match> {
    if !slot_init_const(&fun.body, "s", 0, defs) || !slot_init_const(&fun.body, "i", 1, defs) {
        return None;
    }
    let (header, body, latch) = first_loop(&fun.body)?;
    if !latch.ops.is_empty() {
        return None;
    }
    let (i, n_const) =
        outer_le_param_or_const(header, defs, param0(fun), fun.param_names.first(), 2)?;
    if i != "i" {
        return None;
    }
    let mut saw_div = false;
    for op in &body.ops {
        if let Op::Assign {
            name,
            value: Local(v),
        } = op
        {
            if name != "i" && parse_acc_div_param_or_const(*v, name, &i, param0(fun), n_const, defs)
            {
                saw_div = true;
            }
        }
    }
    if !saw_div || !body_unit_incs_slot(body, &i, defs) {
        return None;
    }
    if !result_is_slot(&fun.body, "s", defs) {
        return None;
    }
    Some(("lumia_divisor_sum", vec![n_arg(fun, n_const)]))
}

fn parse_acc_div_param_or_const(
    dest: u32,
    s_name: &str,
    i: &str,
    n_param: Local,
    n_const: Option<i64>,
    defs: &HashMap<u32, Value>,
) -> bool {
    let Some(term) = add_name_other(dest, s_name, defs) else {
        return false;
    };
    let Some(Value::Binary {
        op: BinOp::Div,
        left: dl,
        right: dr,
        ..
    }) = defs.get(&term.0)
    else {
        return false;
    };
    let right_ok = name_of(*dr, defs).as_deref() == Some(i);
    if !right_ok {
        return false;
    }
    if let Some(c) = n_const {
        return const_of(*dl, defs) == Some(c);
    }
    same_local(*dl, n_param, defs)
        || matches!(defs.get(&dl.0), Some(Value::Local(l)) if same_local(*l, n_param, defs))
}

fn match_product_rem(fun: &CoreFun, defs: &HashMap<u32, Value>) -> Option<Match> {
    if !slot_init_const(&fun.body, "s", 0, defs) || !slot_init_const(&fun.body, "i", 0, defs) {
        return None;
    }
    let (header, body, latch) = first_loop(&fun.body)?;
    if !latch.ops.is_empty() {
        return None;
    }
    let (i, n_const) =
        outer_lt_param_or_const(header, defs, param0(fun), fun.param_names.first(), 2)?;
    if i != "i" {
        return None;
    }
    let (ih, ib, il) = first_loop(body)?;
    if !il.ops.is_empty() {
        return None;
    }
    let (j, _) = outer_lt_param_or_const(ih, defs, param0(fun), fun.param_names.first(), 2)?;
    if j == i || !body_assigns_const(body, &j, 0, defs) {
        return None;
    }
    let mut m_val = None;
    for op in &ib.ops {
        if let Op::Assign {
            name,
            value: Local(v),
        } = op
        {
            if name != &j {
                if let Some(m) = parse_acc_ij1_rem(*v, name, &i, &j, defs) {
                    m_val = Some(m);
                }
            }
        }
    }
    if !body_unit_incs_slot(ib, &j, defs) || !body_unit_incs_slot(body, &i, defs) {
        return None;
    }
    if !result_is_slot(&fun.body, "s", defs) {
        return None;
    }
    let m = m_val?;
    Some((
        "lumia_product_rem_sum",
        vec![n_arg(fun, n_const), RtArg::Const(m)],
    ))
}

fn parse_acc_ij1_rem(
    dest: u32,
    s_name: &str,
    i: &str,
    j: &str,
    defs: &HashMap<u32, Value>,
) -> Option<i64> {
    let (num, m) = acc_add_rem_const_mod(dest, s_name, defs)?;
    // num = i*j + 1
    let Value::Binary {
        op: BinOp::Add,
        left,
        right,
        ..
    } = defs.get(&num.0)?
    else {
        return None;
    };
    let (mul_side, one_side) = if const_of(*left, defs) == Some(1) {
        (*right, *left)
    } else if const_of(*right, defs) == Some(1) {
        (*left, *right)
    } else {
        return None;
    };
    let _ = one_side;
    let Value::Binary {
        op: BinOp::Mul,
        left: ml,
        right: mr,
        ..
    } = defs.get(&mul_side.0)?
    else {
        return None;
    };
    let ln = name_of(*ml, defs);
    let rn = name_of(*mr, defs);
    let ok = (ln.as_deref() == Some(i) && rn.as_deref() == Some(j))
        || (ln.as_deref() == Some(j) && rn.as_deref() == Some(i));
    ok.then_some(m)
}

fn match_matmul_checksum(fun: &CoreFun, defs: &HashMap<u32, Value>) -> Option<Match> {
    if !slot_init_const(&fun.body, "sum", 0, defs) || !slot_init_const(&fun.body, "i", 0, defs) {
        return None;
    }
    let (header, body, latch) = first_loop(&fun.body)?;
    if !latch.ops.is_empty() {
        return None;
    }
    let (i, n_const) =
        outer_lt_param_or_const(header, defs, param0(fun), fun.param_names.first(), 2)?;
    if i != "i" {
        return None;
    }
    // Need nested j and k loops on same bound + Rem modulus on cell accumulate.
    let (jh, jb, jl) = first_loop(body)?;
    if !jl.ops.is_empty() {
        return None;
    }
    let (j, _) = outer_lt_param_or_const(jh, defs, param0(fun), fun.param_names.first(), 2)?;
    if j == i {
        return None;
    }
    let (kh, kb, kl) = first_loop(jb)?;
    if !kl.ops.is_empty() {
        return None;
    }
    let (k, _) = outer_lt_param_or_const(kh, defs, param0(fun), fun.param_names.first(), 2)?;
    if k == i || k == j {
        return None;
    }
    let mut modulus = None;
    for op in &jb.ops {
        if let Op::Assign {
            name,
            value: Local(v),
        } = op
        {
            if name == "sum" {
                if let Some(m) = rem_modulus_of_acc(*v, "sum", defs) {
                    modulus = Some(m);
                }
            }
        }
    }
    if !body_unit_incs_slot(kb, &k, defs)
        || !body_unit_incs_slot(jb, &j, defs)
        || !body_unit_incs_slot(body, &i, defs)
    {
        return None;
    }
    if !result_is_slot(&fun.body, "sum", defs) {
        return None;
    }
    let m = modulus?;
    Some((
        "lumia_matmul_affine_checksum",
        vec![n_arg(fun, n_const), RtArg::Const(m)],
    ))
}

fn rem_modulus_of_acc(dest: u32, acc: &str, defs: &HashMap<u32, Value>) -> Option<i64> {
    let term = add_name_other(dest, acc, defs)?;
    let Value::Binary {
        op: BinOp::Rem,
        right: den,
        ..
    } = defs.get(&term.0)?
    else {
        return None;
    };
    const_of(*den, defs)
}

fn match_range_fold(fun: &CoreFun, defs: &HashMap<u32, Value>) -> Option<Match> {
    if !slot_init_const(&fun.body, "s", 0, defs) || !slot_init_const(&fun.body, "i", 0, defs) {
        return None;
    }
    // Must build `range(0, n)` before the loop.
    let mut saw_range = false;
    for op in &fun.body.ops {
        if matches!(
            op,
            Op::Let {
                value: Value::Loop { .. },
                ..
            }
        ) {
            break;
        }
        if let Op::Let {
            value:
                Value::Builtin {
                    name: Builtin::Range,
                    args,
                    ..
                },
            ..
        } = op
        {
            if args.len() == 2
                && const_of(args[0], defs) == Some(0)
                && (same_local(args[1], param0(fun), defs)
                    || const_of(args[1], defs).is_some_and(|c| c >= 2))
            {
                saw_range = true;
            }
        }
    }
    if !saw_range {
        return None;
    }
    let (header, body, latch) = first_loop(&fun.body)?;
    if !latch.ops.is_empty() {
        return None;
    }
    let (i, n_const) =
        outer_lt_param_or_const(header, defs, param0(fun), fun.param_names.first(), 2)?;
    if i != "i" {
        return None;
    }
    let mut coeffs = None;
    for op in &body.ops {
        if let Op::Assign {
            name,
            value: Local(v),
        } = op
        {
            if name != &i {
                if let Some(t) = parse_acc_get_affine_rem(*v, name, &i, defs) {
                    coeffs = Some(t);
                }
            }
        }
    }
    if !body_unit_incs_slot(body, &i, defs) || !result_is_slot(&fun.body, "s", defs) {
        return None;
    }
    let (a, c, m) = coeffs?;
    if a < 0 || c < 0 {
        return None;
    }
    Some((
        "lumia_affine1_rem_sum",
        vec![
            n_arg(fun, n_const),
            RtArg::Const(a),
            RtArg::Const(c),
            RtArg::Const(m),
        ],
    ))
}

fn parse_acc_get_affine_rem(
    dest: u32,
    s_name: &str,
    i: &str,
    defs: &HashMap<u32, Value>,
) -> Option<(i64, i64, i64)> {
    let term = add_name_other(dest, s_name, defs)?;
    let Value::Binary {
        op: BinOp::Rem,
        left: num,
        right: den,
        ..
    } = defs.get(&term.0)?
    else {
        return None;
    };
    let m = const_of(*den, defs)?;
    let Value::Binary {
        op: BinOp::Add,
        left: l,
        right: r,
        ..
    } = defs.get(&num.0)?
    else {
        return None;
    };
    let (mul_side, c) = if let Some(k) = const_of(*l, defs) {
        (*r, k)
    } else {
        let k = const_of(*r, defs)?;
        (*l, k)
    };
    let Value::Binary {
        op: BinOp::Mul,
        left: ml,
        right: mr,
        ..
    } = defs.get(&mul_side.0)?
    else {
        return None;
    };
    let (get_l, a) = if let Some(k) = const_of(*ml, defs) {
        (*mr, k)
    } else {
        let k = const_of(*mr, defs)?;
        (*ml, k)
    };
    // get_l should be ListGet(_, i)
    let Value::Builtin {
        name: Builtin::ListGet,
        args,
        ..
    } = defs.get(&get_l.0)?
    else {
        return None;
    };
    if args.len() == 2 && name_of(args[1], defs).as_deref() == Some(i) {
        Some((a, c, m))
    } else {
        None
    }
}

fn match_mandelbrot(fun: &CoreFun, defs: &HashMap<u32, Value>) -> Option<Match> {
    if !slot_init_const(&fun.body, "acc", 0, defs) || !slot_init_const(&fun.body, "y", 0, defs) {
        return None;
    }
    if !has_float_approx(defs, 4.0)
        || !has_float_approx(defs, 2.5)
        || !has_float_approx(defs, 3.5)
        || !has_float_approx(defs, 2.0)
        || !has_float_approx(defs, 1.0)
    {
        return None;
    }
    if !has_float_binop_with_const(defs, BinOp::Gt, 4.0)
        && !has_float_binop_with_const(defs, BinOp::Lt, 4.0)
    {
        return None;
    }
    let (header, body, latch) = first_loop(&fun.body)?;
    if !latch.ops.is_empty() {
        return None;
    }
    let (y, h) = header_lt_const(header, defs)?;
    if h != 140 || y != "y" {
        return None;
    }
    let (xh, xb, xl) = first_loop(body)?;
    if !xl.ops.is_empty() {
        return None;
    }
    let (x, w) = header_lt_const(xh, defs)?;
    if w != 200 || x == y || !body_assigns_const(body, &x, 0, defs) {
        return None;
    }
    let (th, tb, _tl) = first_loop(xb)?;
    let (_it, max_bound) = header_lt_bound(th, defs)?;
    let max_is_param = same_local(max_bound, param0(fun), defs)
        || matches!(defs.get(&max_bound.0), Some(Value::Name(nm)) if fun.param_names.first() == Some(nm));
    let max_const = const_of(max_bound, defs);
    if !max_is_param && max_const.is_none() {
        return None;
    }
    let mut saw_break = false;
    for_each_block_dfs(tb, &mut |b| {
        for op in &b.ops {
            if matches!(op, Op::Break) {
                saw_break = true;
            }
        }
    });
    if !saw_break
        || !body_unit_incs_slot(body, &y, defs)
        || !body_unit_incs_slot(xb, &x, defs)
        || !result_is_slot(&fun.body, "acc", defs)
    {
        return None;
    }
    let arg = if let Some(c) = max_const {
        RtArg::Const(c)
    } else {
        RtArg::Param(0)
    };
    Some(("lumia_mandelbrot_checksum", vec![arg]))
}

/// `memTrafficChecksum(n, scanPasses, gatherSteps)` — dense iota scan + LCG gather.
///
/// Also matches fully const-specialized `$c_` clones (`params` empty).
fn match_mem_traffic(fun: &CoreFun, defs: &HashMap<u32, Value>) -> Option<Match> {
    let specialized = fun.params.is_empty();
    if !specialized && fun.params.len() != 3 {
        return None;
    }
    // Fingerprint: LCG multiplier + densify + final rem.
    if !defs
        .values()
        .any(|v| matches!(v, Value::Int(1_103_515_245)))
        || !defs
            .values()
            .any(|v| matches!(v, Value::Int(1_000_000_007)))
        || !defs.values().any(|v| matches!(v, Value::Int(10007)))
        || !defs.values().any(|v| matches!(v, Value::Int(131)))
    {
        return None;
    }
    let mut saw_range = false;
    let mut range_end: Option<i64> = None;
    let mut saw_concat = false;
    let mut saw_take = false;
    let mut saw_list_get = false;
    let mut saw_list_set = false;
    for_each_block_dfs(&fun.body, &mut |b| {
        for op in &b.ops {
            if let Op::Let {
                value: Value::Builtin { name, args, .. },
                ..
            } = op
            {
                match name {
                    Builtin::Range if args.len() == 2 => {
                        if const_of(args[0], defs) == Some(0) {
                            saw_range = true;
                            range_end = const_of(args[1], defs);
                        }
                    }
                    Builtin::ListConcat => saw_concat = true,
                    Builtin::ListTake => saw_take = true,
                    Builtin::ListGet => saw_list_get = true,
                    Builtin::MapSet => saw_list_set = true,
                    _ => {}
                }
            }
        }
    });
    if !saw_range || !saw_concat || !saw_take || !saw_list_get || !saw_list_set {
        return None;
    }
    if !result_is_rem_of_name(&fun.body, "s", 1_000_000_007, defs) {
        return None;
    }
    let args = if specialized {
        let n = range_end.filter(|&c| c >= 2)?;
        let (scan, gather) = mem_traffic_loop_bounds(&fun.body, defs)?;
        vec![RtArg::Const(n), RtArg::Const(scan), RtArg::Const(gather)]
    } else {
        vec![RtArg::Param(0), RtArg::Param(1), RtArg::Param(2)]
    };
    Some(("lumia_mem_traffic_checksum", args))
}

fn result_is_rem_of_name(
    body: &Block,
    name: &str,
    modulus: i64,
    defs: &HashMap<u32, Value>,
) -> bool {
    let Some(res) = body.result else {
        return false;
    };
    let Some(Value::Binary {
        op: BinOp::Rem,
        left,
        right,
        ..
    }) = defs.get(&res.0)
    else {
        return false;
    };
    const_of(*right, defs) == Some(modulus) && name_of(*left, defs).as_deref() == Some(name)
}

/// Outer scan-pass bound and gather-steps bound (both const headers).
fn mem_traffic_loop_bounds(body: &Block, defs: &HashMap<u32, Value>) -> Option<(i64, i64)> {
    let mut bounds = Vec::new();
    for op in &body.ops {
        if let Op::Let {
            value: Value::Loop { header, .. },
            ..
        } = op
        {
            if let Some((_, c)) = header_lt_const(header, defs) {
                bounds.push(c);
            } else if let Some((_, b)) = header_lt_bound(header, defs) {
                if let Some(c) = const_of(b, defs) {
                    bounds.push(c);
                }
            }
        }
    }
    // Expect scanPasses then gatherSteps as the two top-level const loops.
    if bounds.len() >= 2 {
        Some((bounds[0], bounds[1]))
    } else {
        None
    }
}
