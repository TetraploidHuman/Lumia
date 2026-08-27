//! Nested affine rem-accumulate:
//! ```text
//! for i < N {
//!   for j < N {
//!     s = s + ((a * i + b * j + c) % m)
//!   }
//! }
//! ```
//! → `lumi_affine2_rem_sum(N, a, b, c, m)`.

use inkwell::values::{BasicValueEnum, FunctionValue};
use lumi_core::{
    body_iv_unit_inc, const_int, header_lt_const, match_nested_loop, name_of, split_acc_rem, Block,
    Local, Op, Value,
};
use lumi_syntax::BinOp;
use rustc_hash::FxHashMap as HashMap;

use super::super::Codegen;
use anyhow::Result;

#[derive(Debug)]
struct Affine2RemSum {
    s: String,
    i: String,
    n: i64,
    a: i64,
    b: i64,
    c: i64,
    m: i64,
}

impl<'ctx> Codegen<'ctx> {
    pub(crate) fn try_emit_affine2_rem_sum_loop(
        &mut self,
        header: &Block,
        body: &Block,
        latch: &Block,
        _fv: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>> {
        let Some(pat) = match_affine2_rem_sum(header, body, latch, &self.frame.leaf_defs) else {
            return Ok(None);
        };
        if !self.slot_known_eq(&pat.i, 0) || !self.slot_known_eq(&pat.s, 0) {
            return Ok(None);
        }
        let args = [
            self.llvm.i64_ty.const_int(pat.n as u64, true).into(),
            self.llvm.i64_ty.const_int(pat.a as u64, true).into(),
            self.llvm.i64_ty.const_int(pat.b as u64, true).into(),
            self.llvm.i64_ty.const_int(pat.c as u64, true).into(),
            self.llvm.i64_ty.const_int(pat.m as u64, true).into(),
        ];
        Ok(Some(self.emit_rt_n_to_slots_and_zero(
            "lumi_affine2_rem_sum",
            "aff2",
            "affine2_rem_sum result",
            &pat.s,
            &pat.i,
            pat.n,
            &args,
        )?))
    }
}

fn match_affine2_rem_sum(
    header: &Block,
    body: &Block,
    latch: &Block,
    defs: &HashMap<u32, Value>,
) -> Option<Affine2RemSum> {
    let nest = match_nested_loop(header, body, latch, defs, header_lt_const, true, 0)?;
    let i = nest.outer_iv;
    let j = nest.inner_iv;
    let n = nest.n;
    let ib = nest.inner_body;
    // Inner body: s = s + ((a*i + b*j + c) % m); j += 1
    let mut s_name: Option<String> = None;
    let mut coeffs: Option<(i64, i64, i64, i64)> = None;
    for op in &ib.ops {
        if let Op::Assign {
            name,
            value: Local(v),
        } = op
        {
            if let Some(t) = parse_acc_affine_rem(*v, name, &i, &j, defs) {
                s_name = Some(name.clone());
                coeffs = Some(t);
            }
        }
    }
    if !body_iv_unit_inc(ib, &j, defs) || !body_iv_unit_inc(body, &i, defs) {
        return None;
    }
    let (a, b, c, m) = coeffs?;
    // rem_euclid RT matches Lumi `%` only on the nonneg domain.
    if a < 0 || b < 0 || c < 0 {
        return None;
    }
    Some(Affine2RemSum {
        s: s_name?,
        i,
        n,
        a,
        b,
        c,
        m,
    })
}

/// `acc = acc + ((a*i + b*j + c) % m)` — returns `(a,b,c,m)`.
fn parse_acc_affine_rem(
    dest: u32,
    acc: &str,
    i: &str,
    j: &str,
    defs: &HashMap<u32, Value>,
) -> Option<(i64, i64, i64, i64)> {
    let (num, m) = split_acc_rem(dest, acc, defs)?;
    parse_affine3(num, i, j, defs).map(|(a, b, c)| (a, b, c, m))
}

/// `a*i + b*j + c` (any association / order of the three terms).
fn parse_affine3(
    root: Local,
    i: &str,
    j: &str,
    defs: &HashMap<u32, Value>,
) -> Option<(i64, i64, i64)> {
    // Flatten add tree into factors of i, j, and const.
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
                let (cl, nl) = (const_int(*left, defs), name_of(*left, defs));
                let (cr, nr) = (const_int(*right, defs), name_of(*right, defs));
                if let (Some(k), Some(n)) = (cl, nr.as_deref()) {
                    if n == i {
                        *a = a.saturating_add(k);
                        return true;
                    }
                    if n == j {
                        *b = b.saturating_add(k);
                        return true;
                    }
                }
                if let (Some(k), Some(n)) = (cr, nl.as_deref()) {
                    if n == i {
                        *a = a.saturating_add(k);
                        return true;
                    }
                    if n == j {
                        *b = b.saturating_add(k);
                        return true;
                    }
                }
                false
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

#[cfg(test)]
mod match_tests {
    use super::*;
    use crate::emit_value::sr_match_test::bench_cpu_core;

    #[test]
    fn matches_poly_checksum() {
        let core = bench_cpu_core();
        let mut found = 0;
        for fun in &core.functions {
            if !fun.name.contains("poly") && fun.name != "main" {
                continue;
            }
            let defs = crate::nsw_iv::collect_leaf_defs(&fun.body);
            let mut loops = vec![];
            lumi_core::collect_loop_triples(&fun.body, &mut loops);
            for (h, b, l) in &loops {
                if let Some(p) = match_affine2_rem_sum(h, b, l, &defs) {
                    assert_eq!(p.n, 12_000);
                    assert_eq!((p.a, p.b, p.c, p.m), (131, 17, 1, 10007));
                    found += 1;
                }
            }
        }
        assert!(found >= 1, "expected poly affine2 match, got {found}");
    }
}
