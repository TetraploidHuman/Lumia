//! Recognize number-theory accumulations → RT helpers.
//!
//! - nested `ΣΣ gcd(i,j)` (Euclidean may be inlined)
//! - `Σ ⌊n/i⌋`
//! - nested `(i*j+1)%m`
//! - `range` + `get(i)` affine rem fold

use inkwell::values::{BasicValueEnum, FunctionValue};
use lumia_core::{Block, Local, Op, Value};
use lumia_hir::Builtin;
use lumia_core::CoreBinOp as BinOp;
use rustc_hash::FxHashMap as HashMap;

use super::super::Codegen;
use super::sr_pattern::{
    acc_add_rem_const_mod, add_name_other, body_assigns_const, body_assigns_rem, const_of,
    first_direct_loop, header_le_const, header_lt_const, is_add_name_plus_any, is_affine_ik1,
    is_affine_kj1, is_unit_inc, name_ne_zero, name_of,
};
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
        let rt = self.runtime_fn("lumia_gcd_sum")?;
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
        let rt = self.runtime_fn("lumia_divisor_sum")?;
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
        let rt = self.runtime_fn("lumia_product_rem_sum")?;
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
        let rt = self.runtime_fn("lumia_affine1_rem_sum")?;
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
        let rt = self.runtime_fn("lumia_matmul_affine_checksum")?;
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
    let (ih, ib, il) = first_direct_loop(body)?;
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
    let Some(y) = name_ne_zero(res, defs) else {
        return false;
    };
    // body: rem and swap assigns
    body_assigns_rem(body, &y, defs)
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
    let term = add_name_other(dest, s_name, defs)?;
    let Value::Binary {
        op: BinOp::Div,
        left: dl,
        right: dr,
        ..
    } = defs.get(&term.0)?
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
    let (ih, ib, il) = first_direct_loop(body)?;
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
    let (num, m) = acc_add_rem_const_mod(dest, s_name, defs)?;
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
        args, .. } = defs.get(&get_l.0)?
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
        args: rargs, .. } = defs.get(&args[0].0)?
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
    let (jh, jb, jl) = first_direct_loop(body)?;
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
    let (kh, kb, kl) = first_direct_loop(jb)?;
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

fn parse_acc_rem_name(
    dest: u32,
    s_name: &str,
    cell: &str,
    defs: &HashMap<u32, Value>,
) -> Option<i64> {
    let (num, m) = acc_add_rem_const_mod(dest, s_name, defs)?;
    if name_of(num, defs).as_deref() == Some(cell) {
        Some(m)
    } else {
        None
    }
}

#[cfg(test)]
#[path = "number_theory_sr_tests.rs"]
mod match_tests;
