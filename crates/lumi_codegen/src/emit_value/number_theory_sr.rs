//! Recognize number-theory accumulations → RT helpers.
//!
//! - nested `ΣΣ gcd(i,j)` (Euclidean may be inlined)
//! - `Σ ⌊n/i⌋`
//! - nested `(i*j+1)%m`
//! - `range` + `get(i)` affine rem fold

use inkwell::values::{BasicValueEnum, FunctionValue};
use lumi_core::{
    acc_add_has_name, body_assigns_const, body_iv_unit_inc, const_int, first_loop, header_le_const,
    header_lt_const, is_affine_row_col_plus1, is_unit_inc, match_nested_loop, name_of,
    split_acc_add, Block, Local, Op, Value,
};
use lumi_hir::Builtin;
use lumi_syntax::BinOp;
use rustc_hash::FxHashMap as HashMap;

use super::super::Codegen;
use anyhow::Result;

#[derive(Debug)]
struct GcdSum {
    s: String,
    i: String,
    n: i64,
}

#[derive(Debug)]
struct DivisorSum {
    s: String,
    i: String,
    n: i64,
}

#[derive(Debug)]
struct ProductRemSum {
    s: String,
    i: String,
    n: i64,
    m: i64,
}

#[derive(Debug)]
struct RangeAffine1 {
    s: String,
    i: String,
    n: i64,
    a: i64,
    c: i64,
    m: i64,
}

#[derive(Debug)]
struct MatmulAffine {
    sum: String,
    i: String,
    n: i64,
    modulus: i64,
}

impl<'ctx> Codegen<'ctx> {
    pub(crate) fn try_emit_gcd_sum_loop(
        &mut self,
        header: &Block,
        body: &Block,
        latch: &Block,
        _fv: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>> {
        let Some(pat) = match_gcd_sum(header, body, latch, &self.frame.leaf_defs) else {
            return Ok(None);
        };
        if !self.slot_known_eq(&pat.i, 1) || !self.slot_known_eq(&pat.s, 0) {
            return Ok(None);
        }
        Ok(Some(self.emit_rt_n_plus1_to_slots_and_zero(
            "lumi_gcd_sum",
            "gcd_sum",
            "gcd_sum",
            &pat.s,
            &pat.i,
            pat.n,
        )?))
    }

    pub(crate) fn try_emit_divisor_sum_loop(
        &mut self,
        header: &Block,
        body: &Block,
        latch: &Block,
        _fv: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>> {
        let Some(pat) = match_divisor_sum(header, body, latch, &self.frame.leaf_defs) else {
            return Ok(None);
        };
        if !self.slot_known_eq(&pat.i, 1) || !self.slot_known_eq(&pat.s, 0) {
            return Ok(None);
        }
        Ok(Some(self.emit_rt_n_plus1_to_slots_and_zero(
            "lumi_divisor_sum",
            "div_sum",
            "divisor_sum",
            &pat.s,
            &pat.i,
            pat.n,
        )?))
    }

    pub(crate) fn try_emit_product_rem_sum_loop(
        &mut self,
        header: &Block,
        body: &Block,
        latch: &Block,
        _fv: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>> {
        let Some(pat) = match_product_rem_sum(header, body, latch, &self.frame.leaf_defs) else {
            return Ok(None);
        };
        if !self.slot_known_eq(&pat.i, 0) || !self.slot_known_eq(&pat.s, 0) {
            return Ok(None);
        }
        let args = [
            self.llvm.i64_ty.const_int(pat.n as u64, true).into(),
            self.llvm.i64_ty.const_int(pat.m as u64, true).into(),
        ];
        Ok(Some(self.emit_rt_n_to_slots_and_zero(
            "lumi_product_rem_sum",
            "prod_rem",
            "product_rem_sum",
            &pat.s,
            &pat.i,
            pat.n,
            &args,
        )?))
    }

    pub(crate) fn try_emit_range_affine1_loop(
        &mut self,
        header: &Block,
        body: &Block,
        latch: &Block,
        _fv: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>> {
        let Some(pat) = match_range_affine1(header, body, latch, &self.frame.leaf_defs) else {
            return Ok(None);
        };
        if !self.slot_known_eq(&pat.i, 0)
            || !self.slot_known_eq(&pat.s, 0)
            || pat.a < 0
            || pat.c < 0
        {
            return Ok(None);
        }
        let args = [
            self.llvm.i64_ty.const_int(pat.n as u64, true).into(),
            self.llvm.i64_ty.const_int(pat.a as u64, true).into(),
            self.llvm.i64_ty.const_int(pat.c as u64, true).into(),
            self.llvm.i64_ty.const_int(pat.m as u64, true).into(),
        ];
        Ok(Some(self.emit_rt_n_to_slots_and_zero(
            "lumi_affine1_rem_sum",
            "aff1",
            "affine1_rem_sum",
            &pat.s,
            &pat.i,
            pat.n,
            &args,
        )?))
    }

    pub(crate) fn try_emit_matmul_affine_loop(
        &mut self,
        header: &Block,
        body: &Block,
        latch: &Block,
        _fv: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>> {
        let Some(pat) = match_matmul_affine(header, body, latch, &self.frame.leaf_defs) else {
            return Ok(None);
        };
        if !self.slot_known_eq(&pat.i, 0) || !self.slot_known_eq(&pat.sum, 0) {
            return Ok(None);
        }
        let args = [
            self.llvm.i64_ty.const_int(pat.n as u64, true).into(),
            self.llvm.i64_ty.const_int(pat.modulus as u64, true).into(),
        ];
        Ok(Some(self.emit_rt_n_to_slots_and_zero(
            "lumi_matmul_affine_checksum",
            "matmul_aff",
            "matmul_affine_checksum",
            &pat.sum,
            &pat.i,
            pat.n,
            &args,
        )?))
    }
}

fn match_gcd_sum(
    header: &Block,
    body: &Block,
    latch: &Block,
    defs: &HashMap<u32, Value>,
) -> Option<GcdSum> {
    let nest = match_nested_loop(header, body, latch, defs, header_le_const, false, 1)?;
    let i = nest.outer_iv;
    let j = nest.inner_iv;
    let n = nest.n;
    let ib = nest.inner_body;
    let mut saw_euclid = false;
    let mut s_name: Option<String> = None;
    let mut saw_j_inc = false;
    for op in &ib.ops {
        match op {
            Op::Let {
                value:
                    Value::Loop {
                        header: eh,
                        body: eb,
                        latch: el,
                    },
                ..
            } => {
                if is_euclid_loop(eh, eb, el, defs) {
                    saw_euclid = true;
                }
            }
            Op::Assign {
                name,
                value: Local(v),
            } => {
                if name == &j && is_unit_inc(*v, &j, defs) {
                    saw_j_inc = true;
                } else if saw_euclid && acc_add_has_name(*v, name, defs) {
                    s_name = Some(name.clone());
                }
            }
            _ => {}
        }
    }
    if saw_euclid && saw_j_inc && body_iv_unit_inc(body, &i, defs) {
        Some(GcdSum { s: s_name?, i, n })
    } else {
        None
    }
}

fn is_euclid_loop(header: &Block, body: &Block, latch: &Block, defs: &HashMap<u32, Value>) -> bool {
    if !latch.ops.is_empty() {
        return false;
    }
    // header: y != 0
    let Some(res) = header.result else {
        return false;
    };
    let Some(Value::Binary {
        op: BinOp::Ne,
        left,
        right,
        ..
    }) = defs.get(&res.0)
    else {
        return false;
    };
    let y = match (name_of(*left, defs), const_int(*right, defs)) {
        (Some(n), Some(0)) => n,
        (Some(_), _) => match (name_of(*right, defs), const_int(*left, defs)) {
            (Some(n), Some(0)) => n,
            _ => return false,
        },
        _ => return false,
    };
    // body: rem and swap assigns
    let mut saw_rem = false;
    for op in &body.ops {
        if let Op::Assign {
            name,
            value: Local(v),
        } = op
        {
            if name == &y {
                if let Some(Value::Binary { op: BinOp::Rem, .. }) = defs.get(v) {
                    saw_rem = true;
                }
            }
        }
    }
    saw_rem
}

fn match_divisor_sum(
    header: &Block,
    body: &Block,
    latch: &Block,
    defs: &HashMap<u32, Value>,
) -> Option<DivisorSum> {
    if !latch.ops.is_empty() {
        return None;
    }
    let (i, n) = header_le_const(header, defs, false)?;
    if n < 2 {
        return None;
    }
    let mut s_name: Option<String> = None;
    let mut saw_i_inc = false;
    let mut saw_div = false;
    for op in &body.ops {
        if let Op::Assign {
            name,
            value: Local(v),
        } = op
        {
            if name == &i && is_unit_inc(*v, &i, defs) {
                saw_i_inc = true;
            } else if let Some(s) = parse_acc_div_const(*v, name, &i, n, defs) {
                s_name = Some(s);
                saw_div = true;
            }
        }
    }
    if saw_div && saw_i_inc {
        Some(DivisorSum { s: s_name?, i, n })
    } else {
        None
    }
}

/// `s = s + (N / i)` with const N matching loop bound.
fn parse_acc_div_const(
    dest: u32,
    s_name: &str,
    i: &str,
    n: i64,
    defs: &HashMap<u32, Value>,
) -> Option<String> {
    let acc = split_acc_add(dest, s_name, defs)?;
    let Value::Binary {
        op: BinOp::Div,
        left: dl,
        right: dr,
        ..
    } = defs.get(&acc.0)?
    else {
        return None;
    };
    // Only `N / i` (floor-divisor sum). `i / N` is a different series.
    let ok = const_int(*dl, defs) == Some(n) && name_of(*dr, defs).as_deref() == Some(i);
    if ok {
        Some(s_name.to_string())
    } else {
        None
    }
}

fn match_product_rem_sum(
    header: &Block,
    body: &Block,
    latch: &Block,
    defs: &HashMap<u32, Value>,
) -> Option<ProductRemSum> {
    let nest = match_nested_loop(header, body, latch, defs, header_lt_const, false, 0)?;
    let i = nest.outer_iv;
    let j = nest.inner_iv;
    let n = nest.n;
    let ib = nest.inner_body;
    let mut s_name: Option<String> = None;
    let mut m_val: Option<i64> = None;
    let mut saw_j_inc = false;
    for op in &ib.ops {
        if let Op::Assign {
            name,
            value: Local(v),
        } = op
        {
            if name == &j && is_unit_inc(*v, &j, defs) {
                saw_j_inc = true;
            } else if let Some((s, m)) = parse_acc_ij1_rem(*v, name, &i, &j, defs) {
                s_name = Some(s);
                m_val = Some(m);
            }
        }
    }
    if saw_j_inc && body_iv_unit_inc(body, &i, defs) {
        Some(ProductRemSum {
            s: s_name?,
            i,
            n,
            m: m_val?,
        })
    } else {
        None
    }
}

fn parse_acc_ij1_rem(
    dest: u32,
    s_name: &str,
    i: &str,
    j: &str,
    defs: &HashMap<u32, Value>,
) -> Option<(String, i64)> {
    let term = split_acc_add(dest, s_name, defs)?;
    let Value::Binary {
        op: BinOp::Rem,
        left: num,
        right: den,
        ..
    } = defs.get(&term.0)?
    else {
        return None;
    };
    let m = const_int(*den, defs)?;
    if m < 2 {
        return None;
    }
    // num = (i*j + 1)
    let Value::Binary {
        op: BinOp::Add,
        left: a,
        right: b,
        ..
    } = defs.get(&num.0)?
    else {
        return None;
    };
    let (mul_l, one_l) = if const_int(*a, defs) == Some(1) {
        (*b, *a)
    } else if const_int(*b, defs) == Some(1) {
        (*a, *b)
    } else {
        return None;
    };
    let _ = one_l;
    let Value::Binary {
        op: BinOp::Mul,
        left: ml,
        right: mr,
        ..
    } = defs.get(&mul_l.0)?
    else {
        return None;
    };
    let names = (name_of(*ml, defs)?, name_of(*mr, defs)?);
    if (names.0 == i && names.1 == j) || (names.0 == j && names.1 == i) {
        Some((s_name.to_string(), m))
    } else {
        None
    }
}

fn match_range_affine1(
    header: &Block,
    body: &Block,
    latch: &Block,
    defs: &HashMap<u32, Value>,
) -> Option<RangeAffine1> {
    if !latch.ops.is_empty() {
        return None;
    }
    let (i, n) = header_lt_const(header, defs, false)?;
    if n < 2 {
        return None;
    }
    // Function body must define `xs = range(0, n)` — look in leaf_defs for Range builtin
    // feeding ListGet with index i.
    let mut s_name: Option<String> = None;
    let mut coeffs: Option<(i64, i64, i64)> = None;
    let mut saw_i_inc = false;
    for op in &body.ops {
        if let Op::Assign {
            name,
            value: Local(v),
        } = op
        {
            if name == &i && is_unit_inc(*v, &i, defs) {
                saw_i_inc = true;
            } else if let Some(t) = parse_acc_get_affine_rem(*v, name, &i, n, defs) {
                s_name = Some(name.clone());
                coeffs = Some(t);
            }
        }
    }
    if saw_i_inc {
        let (a, c, m) = coeffs?;
        Some(RangeAffine1 {
            s: s_name?,
            i,
            n,
            a,
            c,
            m,
        })
    } else {
        None
    }
}

/// `s = s + ((ListGet(range, i) * a + c) % m)` where get is on `range(0, n)`.
fn parse_acc_get_affine_rem(
    dest: u32,
    s_name: &str,
    i: &str,
    n: i64,
    defs: &HashMap<u32, Value>,
) -> Option<(i64, i64, i64)> {
    let term = split_acc_add(dest, s_name, defs)?;
    let Value::Binary {
        op: BinOp::Rem,
        left: num,
        right: den,
        ..
    } = defs.get(&term.0)?
    else {
        return None;
    };
    let m = const_int(*den, defs)?;
    // num = get*a + c
    let Value::Binary {
        op: BinOp::Add,
        left: l,
        right: r,
        ..
    } = defs.get(&num.0)?
    else {
        return None;
    };
    let (mul_side, c) = if let Some(k) = const_int(*l, defs) {
        (*r, k)
    } else {
        let k = const_int(*r, defs)?;
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
    let (get_l, a) = if let Some(k) = const_int(*ml, defs) {
        (*mr, k)
    } else {
        let k = const_int(*mr, defs)?;
        (*ml, k)
    };
    // get_l must be ListGet(range(0,n), i)
    let Value::Builtin {
        name: Builtin::ListGet,
        args,
    } = defs.get(&get_l.0)?
    else {
        return None;
    };
    if args.len() != 2 {
        return None;
    }
    if name_of(args[1], defs).as_deref() != Some(i) {
        return None;
    }
    let Value::Builtin {
        name: Builtin::Range,
        args: rargs,
    } = defs.get(&args[0].0)?
    else {
        return None;
    };
    if rargs.len() != 2 {
        return None;
    }
    if const_int(rargs[0], defs) != Some(0) {
        return None;
    }
    if const_int(rargs[1], defs) != Some(n) {
        return None;
    }
    Some((a, c, m))
}

/// Triple nest: `cell += (i*n+k+1)*(k*n+j+1)`; `sum += cell % M`.
fn match_matmul_affine(
    header: &Block,
    body: &Block,
    latch: &Block,
    defs: &HashMap<u32, Value>,
) -> Option<MatmulAffine> {
    let nest = match_nested_loop(header, body, latch, defs, header_lt_const, false, 0)?;
    let i = nest.outer_iv;
    let j = nest.inner_iv;
    let n = nest.n;
    let jb = nest.inner_body;
    let (kh, kb, kl) = first_loop(jb)?;
    if !kl.ops.is_empty() {
        return None;
    }
    let (k, n3) = header_lt_const(kh, defs, false)?;
    if n3 != n || k == i || k == j {
        return None;
    }
    if !body_assigns_const(jb, &k, 0, defs) {
        return None;
    }
    // k-body: cell += (i*n+k+1)*(k*n+j+1); k += 1
    let mut cell_name: Option<String> = None;
    let mut saw_k_inc = false;
    for op in &kb.ops {
        if let Op::Assign {
            name,
            value: Local(v),
        } = op
        {
            if name == &k && is_unit_inc(*v, &k, defs) {
                saw_k_inc = true;
            } else if is_matmul_cell_acc(*v, name, &i, &j, &k, n, defs) {
                cell_name = Some(name.clone());
            }
        }
    }
    let cell = cell_name?;
    // j-body after k-loop: sum += cell % M; j += 1
    let mut sum_name: Option<String> = None;
    let mut modulus: Option<i64> = None;
    for op in &jb.ops {
        if let Op::Assign {
            name,
            value: Local(v),
        } = op
        {
            if let Some(m) = parse_acc_rem_name(*v, name, &cell, defs) {
                sum_name = Some(name.clone());
                modulus = Some(m);
            }
        }
    }
    if saw_k_inc && body_iv_unit_inc(jb, &j, defs) && body_iv_unit_inc(body, &i, defs) {
        Some(MatmulAffine {
            sum: sum_name?,
            i,
            n,
            modulus: modulus?,
        })
    } else {
        None
    }
}

fn is_matmul_cell_acc(
    dest: u32,
    cell: &str,
    i: &str,
    j: &str,
    k: &str,
    n: i64,
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
    let prod = if name_of(*left, defs).as_deref() == Some(cell) {
        *right
    } else if name_of(*right, defs).as_deref() == Some(cell) {
        *left
    } else {
        return false;
    };
    let Some(Value::Binary {
        op: BinOp::Mul,
        left: a,
        right: b,
        ..
    }) = defs.get(&prod.0)
    else {
        return false;
    };
    is_affine_row_col_plus1(*a, i, k, n, defs) && is_affine_row_col_plus1(*b, k, j, n, defs)
        || is_affine_row_col_plus1(*b, i, k, n, defs) && is_affine_row_col_plus1(*a, k, j, n, defs)
}

fn parse_acc_rem_name(
    dest: u32,
    s_name: &str,
    cell: &str,
    defs: &HashMap<u32, Value>,
) -> Option<i64> {
    let term = split_acc_add(dest, s_name, defs)?;
    let Value::Binary {
        op: BinOp::Rem,
        left: num,
        right: den,
        ..
    } = defs.get(&term.0)?
    else {
        return None;
    };
    if name_of(*num, defs).as_deref() != Some(cell) {
        return None;
    }
    let m = const_int(*den, defs)?;
    if m < 2 {
        None
    } else {
        Some(m)
    }
}

#[cfg(test)]
mod match_tests {
    use super::*;
    use crate::emit_value::sr_match_test::{bench_cpu_core, count_loop_matches};

    #[test]
    fn matches_new_bench_srs() {
        let core = bench_cpu_core();
        assert!(
            count_loop_matches(&core, |h, b, l, d| match_gcd_sum(h, b, l, d).is_some()) >= 1,
            "gcd"
        );
        assert!(
            count_loop_matches(&core, |h, b, l, d| match_divisor_sum(h, b, l, d).is_some()) >= 1,
            "div"
        );
        assert!(
            count_loop_matches(&core, |h, b, l, d| {
                match_product_rem_sum(h, b, l, d).is_some()
            }) >= 1,
            "prod"
        );
        assert!(
            count_loop_matches(&core, |h, b, l, d| {
                match_range_affine1(h, b, l, d).is_some()
            }) >= 1,
            "range"
        );
        assert!(
            count_loop_matches(&core, |h, b, l, d| match_matmul_affine(h, b, l, d)
                .is_some())
                >= 1,
            "matmul"
        );
    }
}
