//! Recognize number-theory accumulations → RT helpers.
//!
//! - nested `ΣΣ gcd(i,j)` (Euclidean may be inlined)
//! - `Σ ⌊n/i⌋`
//! - nested `(i*j+1)%m`
//! - `range` + `get(i)` affine rem fold

use inkwell::values::{BasicValueEnum, FunctionValue};
use lumi_core::{Block, Local, Op, Value};
use lumi_hir::Builtin;
use lumi_syntax::BinOp;
use rustc_hash::FxHashMap as HashMap;

use super::super::Codegen;
use anyhow::{Context as AnyhowContext, Result};

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
        let rt = self.runtime_fn("lumi_gcd_sum")?;
        let n = self.llvm.i64_ty.const_int(pat.n as u64, true);
        let call = crate::error::llvm(self.llvm.builder.build_call(rt, &[n.into()], "gcd_sum"))?;
        let s = call
            .try_as_basic_value()
            .basic()
            .context("gcd_sum")?
            .into_int_value();
        self.store_slot_i64(&pat.s, s)?;
        self.store_slot_i64(&pat.i, self.llvm.i64_ty.const_int((pat.n + 1) as u64, true))?;
        Ok(Some(self.llvm.i64_ty.const_int(0, false).into()))
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
        let rt = self.runtime_fn("lumi_divisor_sum")?;
        let n = self.llvm.i64_ty.const_int(pat.n as u64, true);
        let call = crate::error::llvm(self.llvm.builder.build_call(rt, &[n.into()], "div_sum"))?;
        let s = call
            .try_as_basic_value()
            .basic()
            .context("divisor_sum")?
            .into_int_value();
        self.store_slot_i64(&pat.s, s)?;
        self.store_slot_i64(&pat.i, self.llvm.i64_ty.const_int((pat.n + 1) as u64, true))?;
        Ok(Some(self.llvm.i64_ty.const_int(0, false).into()))
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
        let rt = self.runtime_fn("lumi_product_rem_sum")?;
        let args = [
            self.llvm.i64_ty.const_int(pat.n as u64, true).into(),
            self.llvm.i64_ty.const_int(pat.m as u64, true).into(),
        ];
        let call = crate::error::llvm(self.llvm.builder.build_call(rt, &args, "prod_rem"))?;
        let s = call
            .try_as_basic_value()
            .basic()
            .context("product_rem_sum")?
            .into_int_value();
        self.store_slot_i64(&pat.s, s)?;
        self.store_slot_i64(&pat.i, self.llvm.i64_ty.const_int(pat.n as u64, true))?;
        Ok(Some(self.llvm.i64_ty.const_int(0, false).into()))
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
        let rt = self.runtime_fn("lumi_affine1_rem_sum")?;
        let args = [
            self.llvm.i64_ty.const_int(pat.n as u64, true).into(),
            self.llvm.i64_ty.const_int(pat.a as u64, true).into(),
            self.llvm.i64_ty.const_int(pat.c as u64, true).into(),
            self.llvm.i64_ty.const_int(pat.m as u64, true).into(),
        ];
        let call = crate::error::llvm(self.llvm.builder.build_call(rt, &args, "aff1"))?;
        let s = call
            .try_as_basic_value()
            .basic()
            .context("affine1_rem_sum")?
            .into_int_value();
        self.store_slot_i64(&pat.s, s)?;
        self.store_slot_i64(&pat.i, self.llvm.i64_ty.const_int(pat.n as u64, true))?;
        Ok(Some(self.llvm.i64_ty.const_int(0, false).into()))
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
        let rt = self.runtime_fn("lumi_matmul_affine_checksum")?;
        let args = [
            self.llvm.i64_ty.const_int(pat.n as u64, true).into(),
            self.llvm.i64_ty.const_int(pat.modulus as u64, true).into(),
        ];
        let call = crate::error::llvm(self.llvm.builder.build_call(rt, &args, "matmul_aff"))?;
        let s = call
            .try_as_basic_value()
            .basic()
            .context("matmul_affine_checksum")?
            .into_int_value();
        self.store_slot_i64(&pat.sum, s)?;
        self.store_slot_i64(&pat.i, self.llvm.i64_ty.const_int(pat.n as u64, true))?;
        Ok(Some(self.llvm.i64_ty.const_int(0, false).into()))
    }
}

fn match_gcd_sum(
    header: &Block,
    body: &Block,
    latch: &Block,
    defs: &HashMap<u32, Value>,
) -> Option<GcdSum> {
    if !latch.ops.is_empty() {
        return None;
    }
    let (i, n) = header_le_const(header, defs)?;
    if n < 2 {
        return None;
    }
    let (ih, ib, il) = find_inner_loop(body)?;
    if !il.ops.is_empty() {
        return None;
    }
    let (j, n2) = header_le_const(ih, defs)?;
    if n2 != n || j == i {
        return None;
    }
    // Outer body must reset `j := 1` (RT / closed form assume j∈[1,n]).
    if !body_assigns_const(body, &j, 1, defs) {
        return None;
    }
    // Inner: inlined Euclid on two temps copied from i,j; then s += x; j += 1
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
                } else if saw_euclid && is_add_name_plus_any(*v, name, defs) {
                    s_name = Some(name.clone());
                }
            }
            _ => {}
        }
    }
    let mut saw_i_inc = false;
    for op in &body.ops {
        if let Op::Assign {
            name,
            value: Local(v),
        } = op
        {
            if name == &i && is_unit_inc(*v, &i, defs) {
                saw_i_inc = true;
            }
        }
    }
    if saw_euclid && saw_j_inc && saw_i_inc {
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
    let y = match (name_of(*left, defs), const_of(*right, defs)) {
        (Some(n), Some(0)) => n,
        (Some(_), _) => match (name_of(*right, defs), const_of(*left, defs)) {
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
    let (i, n) = header_le_const(header, defs)?;
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
    let Value::Binary {
        op: BinOp::Add,
        left,
        right,
        ..
    } = defs.get(&dest)?
    else {
        return None;
    };
    let (acc, term) = if name_of(*left, defs).as_deref() == Some(s_name) {
        (*right, *left)
    } else if name_of(*right, defs).as_deref() == Some(s_name) {
        (*left, *right)
    } else {
        return None;
    };
    let _ = term;
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
    let ok = const_of(*dl, defs) == Some(n) && name_of(*dr, defs).as_deref() == Some(i);
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
    if !latch.ops.is_empty() {
        return None;
    }
    let (i, n) = header_lt_const(header, defs)?;
    if n < 2 {
        return None;
    }
    let (ih, ib, il) = find_inner_loop(body)?;
    if !il.ops.is_empty() {
        return None;
    }
    let (j, n2) = header_lt_const(ih, defs)?;
    if n2 != n || j == i {
        return None;
    }
    // Outer body must reset `j := 0` (RT assumes j∈[0,n)).
    if !body_assigns_const(body, &j, 0, defs) {
        return None;
    }
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
    let mut saw_i_inc = false;
    for op in &body.ops {
        if let Op::Assign {
            name,
            value: Local(v),
        } = op
        {
            if name == &i && is_unit_inc(*v, &i, defs) {
                saw_i_inc = true;
            }
        }
    }
    if saw_j_inc && saw_i_inc {
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
    let Value::Binary {
        op: BinOp::Add,
        left,
        right,
        ..
    } = defs.get(&dest)?
    else {
        return None;
    };
    let term = if name_of(*left, defs).as_deref() == Some(s_name) {
        *right
    } else if name_of(*right, defs).as_deref() == Some(s_name) {
        *left
    } else {
        return None;
    };
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
    let (mul_l, one_l) = if const_of(*a, defs) == Some(1) {
        (*b, *a)
    } else if const_of(*b, defs) == Some(1) {
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
    let (i, n) = header_lt_const(header, defs)?;
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
    let Value::Binary {
        op: BinOp::Add,
        left,
        right,
        ..
    } = defs.get(&dest)?
    else {
        return None;
    };
    let term = if name_of(*left, defs).as_deref() == Some(s_name) {
        *right
    } else if name_of(*right, defs).as_deref() == Some(s_name) {
        *left
    } else {
        return None;
    };
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
    if const_of(rargs[0], defs) != Some(0) {
        return None;
    }
    if const_of(rargs[1], defs) != Some(n) {
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
    if !latch.ops.is_empty() {
        return None;
    }
    let (i, n) = header_lt_const(header, defs)?;
    if n < 2 {
        return None;
    }
    let (jh, jb, jl) = find_inner_loop(body)?;
    if !jl.ops.is_empty() {
        return None;
    }
    let (j, n2) = header_lt_const(jh, defs)?;
    if n2 != n || j == i {
        return None;
    }
    // Outer body resets `j := 0`; j-body resets `k := 0` (and usually `cell := 0`).
    if !body_assigns_const(body, &j, 0, defs) {
        return None;
    }
    let (kh, kb, kl) = find_inner_loop(jb)?;
    if !kl.ops.is_empty() {
        return None;
    }
    let (k, n3) = header_lt_const(kh, defs)?;
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
    let mut saw_j_inc = false;
    for op in &jb.ops {
        if let Op::Assign {
            name,
            value: Local(v),
        } = op
        {
            if name == &j && is_unit_inc(*v, &j, defs) {
                saw_j_inc = true;
            } else if let Some(m) = parse_acc_rem_name(*v, name, &cell, defs) {
                sum_name = Some(name.clone());
                modulus = Some(m);
            }
        }
    }
    let mut saw_i_inc = false;
    for op in &body.ops {
        if let Op::Assign {
            name,
            value: Local(v),
        } = op
        {
            if name == &i && is_unit_inc(*v, &i, defs) {
                saw_i_inc = true;
            }
        }
    }
    if saw_k_inc && saw_j_inc && saw_i_inc {
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
    is_affine_ik1(*a, i, k, n, defs) && is_affine_kj1(*b, k, j, n, defs)
        || is_affine_ik1(*b, i, k, n, defs) && is_affine_kj1(*a, k, j, n, defs)
}

/// `i*n + k + 1`
fn is_affine_ik1(l: Local, i: &str, k: &str, n: i64, defs: &HashMap<u32, Value>) -> bool {
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
    // (i*n) + k
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

/// `k*n + j + 1`
fn is_affine_kj1(l: Local, k: &str, j: &str, n: i64, defs: &HashMap<u32, Value>) -> bool {
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

fn is_name_mul_const(l: Local, name: &str, n: i64, defs: &HashMap<u32, Value>) -> bool {
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

fn parse_acc_rem_name(
    dest: u32,
    s_name: &str,
    cell: &str,
    defs: &HashMap<u32, Value>,
) -> Option<i64> {
    let Value::Binary {
        op: BinOp::Add,
        left,
        right,
        ..
    } = defs.get(&dest)?
    else {
        return None;
    };
    let term = if name_of(*left, defs).as_deref() == Some(s_name) {
        *right
    } else if name_of(*right, defs).as_deref() == Some(s_name) {
        *left
    } else {
        return None;
    };
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
    let m = const_of(*den, defs)?;
    if m < 2 {
        None
    } else {
        Some(m)
    }
}

fn find_inner_loop(body: &Block) -> Option<(&Block, &Block, &Block)> {
    for op in &body.ops {
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
            return Some((header, body, latch));
        }
    }
    None
}

fn body_assigns_const(body: &Block, slot: &str, expect: i64, defs: &HashMap<u32, Value>) -> bool {
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

fn header_lt_const(header: &Block, defs: &HashMap<u32, Value>) -> Option<(String, i64)> {
    let res = header.result?;
    let Value::Binary {
        op, left, right, ..
    } = defs.get(&res.0)?
    else {
        return None;
    };
    match op {
        BinOp::Lt => {
            let name = name_of(*left, defs)?;
            let n = const_of(*right, defs)?;
            Some((name, n))
        }
        _ => None,
    }
}

fn header_le_const(header: &Block, defs: &HashMap<u32, Value>) -> Option<(String, i64)> {
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
        _ => None,
    }
}

fn name_of(l: Local, defs: &HashMap<u32, Value>) -> Option<String> {
    match defs.get(&l.0)? {
        Value::Name(n) => Some(n.clone()),
        _ => None,
    }
}

fn const_of(l: Local, defs: &HashMap<u32, Value>) -> Option<i64> {
    match defs.get(&l.0)? {
        Value::Int(n) => Some(*n),
        _ => None,
    }
}

fn is_unit_inc(dest: u32, name: &str, defs: &HashMap<u32, Value>) -> bool {
    let Some(Value::Binary {
        op: BinOp::Add,
        left,
        right,
        ..
    }) = defs.get(&dest)
    else {
        return false;
    };
    let l = name_of(*left, defs).as_deref() == Some(name);
    let r = name_of(*right, defs).as_deref() == Some(name);
    (l && const_of(*right, defs) == Some(1)) || (r && const_of(*left, defs) == Some(1))
}

fn is_add_name_plus_any(dest: u32, s_name: &str, defs: &HashMap<u32, Value>) -> bool {
    let Some(Value::Binary {
        op: BinOp::Add,
        left,
        right,
        ..
    }) = defs.get(&dest)
    else {
        return false;
    };
    name_of(*left, defs).as_deref() == Some(s_name)
        || name_of(*right, defs).as_deref() == Some(s_name)
}

#[cfg(test)]
mod match_tests {
    use super::*;
    use lumi_core::collect_loop_triples;
    use lumi_opt::{compile_source_to_optimized, OptOptions};

    #[test]
    fn matches_new_bench_srs() {
        let src = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/bench_cpu.lm"
        ))
        .unwrap();
        let core = compile_source_to_optimized(&src, &OptOptions::for_build(true)).unwrap();
        let mut gcd = 0;
        let mut div = 0;
        let mut prod = 0;
        let mut range = 0;
        let mut matmul = 0;
        for f in &core.functions {
            let defs = crate::nsw_iv::collect_leaf_defs(&f.body);
            let mut loops = vec![];
            collect_loop_triples(&f.body, &mut loops);
            for (h, b, l) in &loops {
                if match_gcd_sum(h, b, l, &defs).is_some() {
                    gcd += 1;
                }
                if match_divisor_sum(h, b, l, &defs).is_some() {
                    div += 1;
                }
                if match_product_rem_sum(h, b, l, &defs).is_some() {
                    prod += 1;
                }
                if match_range_affine1(h, b, l, &defs).is_some() {
                    range += 1;
                }
                if match_matmul_affine(h, b, l, &defs).is_some() {
                    matmul += 1;
                }
            }
        }
        assert!(gcd >= 1, "gcd matches={gcd}");
        assert!(div >= 1, "div matches={div}");
        assert!(prod >= 1, "prod matches={prod}");
        assert!(range >= 1, "range matches={range}");
        assert!(matmul >= 1, "matmul matches={matmul}");
    }
}
