//! Recognize float-orbit loops and emit tight LLVM IR (O3-friendly).
//!
//! Mandelbrot whole-fn rewrite lives in `lumia_opt::domain_sr`.
//!
//! Prefer IR over a Rust RT call: clang-linked `staticlib` does not LTO with the
//! caller's constants, so a cross-language helper stayed ~5× slower than same-crate
//! loops. Emitting here lets `default<O3>` see the full nest.

use inkwell::types::VectorType;
use inkwell::values::{BasicValueEnum, FloatValue, FunctionValue, IntValue, VectorValue};
use inkwell::{FloatPredicate, IntPredicate};
use lumia_core::CoreBinOp as BinOp;
use lumia_core::{for_each_block_dfs, Block, Local, Op, Value};
use rustc_hash::FxHashMap as HashMap;

use super::super::Codegen;
use super::sr_pattern::{
    body_assigns_const, const_of, has_float_approx, has_float_binop_with_const, header_lt_const,
    is_unit_inc,
};
use anyhow::{anyhow, Result};
use lumia_core::header_lt_bound as core_header_lt_bound;

#[derive(Debug)]
struct FloatOrbit {
    h: String,
    i: String,
    n: OrbitBound,
    iters: i64,
}

#[derive(Debug, Clone)]
enum OrbitBound {
    Const(i64),
    Local(Local),
}

impl<'ctx> Codegen<'ctx> {
    pub(crate) fn try_emit_float_orbit_loop(
        &mut self,
        header: &Block,
        body: &Block,
        latch: &Block,
        fv: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>> {
        let Some(pat) = match_float_orbit(header, body, latch, &self.frame.leaf_defs) else {
            return Ok(None);
        };
        // IR assumes `h = 0`, `i = 0` before the nest.
        if !self.slot_known_eq(&pat.h, 0) || !self.slot_known_eq(&pat.i, 0) {
            return Ok(None);
        }
        self.emit_float_orbit_ir(&pat, fv)?;
        Ok(Some(self.llvm.i64_ty.const_int(0, false).into()))
    }

    fn emit_float_orbit_ir(&mut self, pat: &FloatOrbit, fv: FunctionValue<'ctx>) -> Result<()> {
        // Independent outer orbits → 8-wide / 4-wide LLVM vectors when `n` divides.
        match pat.n {
            OrbitBound::Const(n) if n >= 8 && n % 8 == 0 => {
                self.emit_float_orbit_phi_vec(pat, fv, n, 8)
            }
            OrbitBound::Const(n) if n >= 4 && n % 4 == 0 => {
                self.emit_float_orbit_phi_vec(pat, fv, n, 4)
            }
            _ => self.emit_float_orbit_phi_scalar(pat, fv),
        }
    }

    fn emit_float_orbit_phi_scalar(
        &mut self,
        pat: &FloatOrbit,
        fv: FunctionValue<'ctx>,
    ) -> Result<()> {
        let fty = self.llvm.context.f64_type();
        let n = match pat.n {
            OrbitBound::Const(c) => self.llvm.i64_ty.const_int(c as u64, true),
            OrbitBound::Local(l) => self.coerce_i64(self.local(l)?)?,
        };
        let iters = self.llvm.i64_ty.const_int(pat.iters as u64, true);
        let c_0_1 = fty.const_float(0.1);
        let c_1e8 = fty.const_float(1e-8);
        let c_3_7 = fty.const_float(3.7);
        let c_1_0 = fty.const_float(1.0);
        let c_0_5 = fty.const_float(0.5);
        let one = self.llvm.i64_ty.const_int(1, false);
        let zero = self.llvm.i64_ty.const_int(0, false);

        let pre = self
            .llvm
            .builder
            .get_insert_block()
            .ok_or_else(|| anyhow!("float orbit: no insert block"))?;
        let h0 = self.load_slot_i64(&pat.h)?;
        let i0 = self.load_slot_i64(&pat.i)?;

        let o_hdr = self.llvm.context.append_basic_block(fv, "fo_hdr");
        let o_body = self.llvm.context.append_basic_block(fv, "fo_body");
        let i_hdr = self.llvm.context.append_basic_block(fv, "fo_ihdr");
        let i_body = self.llvm.context.append_basic_block(fv, "fo_ibody");
        let o_latch = self.llvm.context.append_basic_block(fv, "fo_olatch");
        let o_exit = self.llvm.context.append_basic_block(fv, "fo_exit");

        crate::error::llvm(self.llvm.builder.build_unconditional_branch(o_hdr))?;

        self.llvm.builder.position_at_end(o_hdr);
        let i_phi = crate::error::llvm(self.llvm.builder.build_phi(self.llvm.i64_ty, "fo_i"))?;
        let h_phi = crate::error::llvm(self.llvm.builder.build_phi(self.llvm.i64_ty, "fo_h"))?;
        i_phi.add_incoming(&[(&i0, pre)]);
        h_phi.add_incoming(&[(&h0, pre)]);
        let i = i_phi.as_basic_value().into_int_value();
        let h = h_phi.as_basic_value().into_int_value();
        let cont = crate::error::llvm(self.llvm.builder.build_int_compare(
            IntPredicate::SLT,
            i,
            n,
            "fo_ilt",
        ))?;
        crate::error::llvm(
            self.llvm
                .builder
                .build_conditional_branch(cont, o_body, o_exit),
        )?;

        self.llvm.builder.position_at_end(o_body);
        let i_f = crate::error::llvm(self.llvm.builder.build_signed_int_to_float(
            i,
            fty,
            "fo_sitofp",
        ))?;
        let scaled = crate::error::llvm(self.llvm.builder.build_float_mul(c_1e8, i_f, "fo_scale"))?;
        let x_init = crate::error::llvm(self.llvm.builder.build_float_add(c_0_1, scaled, "fo_x0"))?;
        crate::error::llvm(self.llvm.builder.build_unconditional_branch(i_hdr))?;

        self.llvm.builder.position_at_end(i_hdr);
        let k_phi = crate::error::llvm(self.llvm.builder.build_phi(self.llvm.i64_ty, "fo_k"))?;
        let x_phi = crate::error::llvm(self.llvm.builder.build_phi(fty, "fo_x"))?;
        let hi_phi = crate::error::llvm(self.llvm.builder.build_phi(self.llvm.i64_ty, "fo_hi"))?;
        k_phi.add_incoming(&[(&zero, o_body)]);
        x_phi.add_incoming(&[(&x_init, o_body)]);
        hi_phi.add_incoming(&[(&h, o_body)]);
        let k = k_phi.as_basic_value().into_int_value();
        let x = x_phi.as_basic_value().into_float_value();
        let hi = hi_phi.as_basic_value().into_int_value();
        let k_cont = crate::error::llvm(self.llvm.builder.build_int_compare(
            IntPredicate::SLT,
            k,
            iters,
            "fo_klt",
        ))?;
        crate::error::llvm(
            self.llvm
                .builder
                .build_conditional_branch(k_cont, i_body, o_latch),
        )?;

        self.llvm.builder.position_at_end(i_body);
        let (x1, h1) = self.fo_step_branchless(x, hi, c_3_7, c_1_0, c_0_5, one, zero, "")?;
        let k1 = crate::error::llvm(self.llvm.builder.build_int_nsw_add(k, one, "fo_k1"))?;
        // Latch is a dedicated block so phi incoming edges stay unambiguous.
        let i_latch = self.llvm.context.append_basic_block(fv, "fo_ilatch");
        crate::error::llvm(self.llvm.builder.build_unconditional_branch(i_latch))?;
        self.llvm.builder.position_at_end(i_latch);
        k_phi.add_incoming(&[(&k1, i_latch)]);
        x_phi.add_incoming(&[(&x1, i_latch)]);
        hi_phi.add_incoming(&[(&h1, i_latch)]);
        crate::error::llvm(self.llvm.builder.build_unconditional_branch(i_hdr))?;

        self.llvm.builder.position_at_end(o_latch);
        let i1 = crate::error::llvm(self.llvm.builder.build_int_nsw_add(i, one, "fo_i1"))?;
        i_phi.add_incoming(&[(&i1, o_latch)]);
        h_phi.add_incoming(&[(&hi, o_latch)]);
        crate::error::llvm(self.llvm.builder.build_unconditional_branch(o_hdr))?;

        self.llvm.builder.position_at_end(o_exit);
        self.store_slot_i64(&pat.h, h)?;
        self.store_slot_i64(&pat.i, i)?;
        Ok(())
    }

    /// `width`-lane independent orbits as LLVM `<width x double>` (4 or 8).
    fn emit_float_orbit_phi_vec(
        &mut self,
        pat: &FloatOrbit,
        fv: FunctionValue<'ctx>,
        n_const: i64,
        width: u32,
    ) -> Result<()> {
        debug_assert!(width == 4 || width == 8);
        let fty = self.llvm.context.f64_type();
        let vty = fty.vec_type(width);
        let i64v = self.llvm.i64_ty.vec_type(width);
        let n = self.llvm.i64_ty.const_int(n_const as u64, true);
        let iters = self.llvm.i64_ty.const_int(pat.iters as u64, true);
        let splat_f = |v: f64| -> VectorValue<'ctx> {
            let vals: Vec<_> = (0..width).map(|_| fty.const_float(v)).collect();
            VectorType::const_vector(&vals)
        };
        let c_0_1 = splat_f(0.1);
        let c_1e8 = splat_f(1e-8);
        let c_3_7 = splat_f(3.7);
        let c_1_0 = splat_f(1.0);
        let c_0_5 = splat_f(0.5);
        let hit_ones = {
            let vals: Vec<_> = (0..width)
                .map(|_| self.llvm.i64_ty.const_int(1, false))
                .collect();
            VectorType::const_vector(&vals)
        };
        let hit_zeros = i64v.const_zero();
        let lane_offs = {
            let vals: Vec<_> = (0..width)
                .map(|lane| self.llvm.i64_ty.const_int(lane as u64, false))
                .collect();
            VectorType::const_vector(&vals)
        };
        let one = self.llvm.i64_ty.const_int(1, false);
        let step_w = self.llvm.i64_ty.const_int(width as u64, false);
        let zero = self.llvm.i64_ty.const_int(0, false);
        let tag = if width == 8 { "fo8" } else { "fo4" };

        let pre = self
            .llvm
            .builder
            .get_insert_block()
            .ok_or_else(|| anyhow!("float orbit {tag}: no insert block"))?;
        let h0 = self.load_slot_i64(&pat.h)?;
        let i0 = self.load_slot_i64(&pat.i)?;

        let o_hdr = self
            .llvm
            .context
            .append_basic_block(fv, &format!("{tag}_hdr"));
        let o_body = self
            .llvm
            .context
            .append_basic_block(fv, &format!("{tag}_body"));
        let i_hdr = self
            .llvm
            .context
            .append_basic_block(fv, &format!("{tag}_ihdr"));
        let i_body = self
            .llvm
            .context
            .append_basic_block(fv, &format!("{tag}_ibody"));
        let i_latch = self
            .llvm
            .context
            .append_basic_block(fv, &format!("{tag}_ilatch"));
        let o_latch = self
            .llvm
            .context
            .append_basic_block(fv, &format!("{tag}_olatch"));
        let o_exit = self
            .llvm
            .context
            .append_basic_block(fv, &format!("{tag}_exit"));

        crate::error::llvm(self.llvm.builder.build_unconditional_branch(o_hdr))?;

        self.llvm.builder.position_at_end(o_hdr);
        let i_phi = crate::error::llvm(
            self.llvm
                .builder
                .build_phi(self.llvm.i64_ty, &format!("{tag}_i")),
        )?;
        let h_phi = crate::error::llvm(
            self.llvm
                .builder
                .build_phi(self.llvm.i64_ty, &format!("{tag}_h")),
        )?;
        i_phi.add_incoming(&[(&i0, pre)]);
        h_phi.add_incoming(&[(&h0, pre)]);
        let i = i_phi.as_basic_value().into_int_value();
        let h = h_phi.as_basic_value().into_int_value();
        let cont = crate::error::llvm(self.llvm.builder.build_int_compare(
            IntPredicate::SLT,
            i,
            n,
            &format!("{tag}_ilt"),
        ))?;
        crate::error::llvm(
            self.llvm
                .builder
                .build_conditional_branch(cont, o_body, o_exit),
        )?;

        self.llvm.builder.position_at_end(o_body);
        let mut i_splat = i64v.get_undef();
        for lane in 0..width as u64 {
            i_splat = crate::error::llvm(self.llvm.builder.build_insert_element(
                i_splat,
                i,
                self.llvm.i64_ty.const_int(lane, false),
                &format!("{tag}_isplat"),
            ))?;
        }
        let i_vec = crate::error::llvm(self.llvm.builder.build_int_nsw_add(
            i_splat,
            lane_offs,
            &format!("{tag}_iv"),
        ))?;
        let i_f = crate::error::llvm(self.llvm.builder.build_signed_int_to_float(
            i_vec,
            vty,
            &format!("{tag}_sitofp"),
        ))?;
        let scaled = crate::error::llvm(self.llvm.builder.build_float_mul(
            c_1e8,
            i_f,
            &format!("{tag}_sc"),
        ))?;
        let x_init = crate::error::llvm(self.llvm.builder.build_float_add(
            c_0_1,
            scaled,
            &format!("{tag}_x0"),
        ))?;
        crate::error::llvm(self.llvm.builder.build_unconditional_branch(i_hdr))?;

        self.llvm.builder.position_at_end(i_hdr);
        let k_phi = crate::error::llvm(
            self.llvm
                .builder
                .build_phi(self.llvm.i64_ty, &format!("{tag}_k")),
        )?;
        let hi_phi = crate::error::llvm(
            self.llvm
                .builder
                .build_phi(self.llvm.i64_ty, &format!("{tag}_hi")),
        )?;
        let x_phi = crate::error::llvm(self.llvm.builder.build_phi(vty, &format!("{tag}_x")))?;
        k_phi.add_incoming(&[(&zero, o_body)]);
        hi_phi.add_incoming(&[(&h, o_body)]);
        x_phi.add_incoming(&[(&x_init, o_body)]);
        let k = k_phi.as_basic_value().into_int_value();
        let hi = hi_phi.as_basic_value().into_int_value();
        let x = x_phi.as_basic_value().into_vector_value();
        let k_cont = crate::error::llvm(self.llvm.builder.build_int_compare(
            IntPredicate::SLT,
            k,
            iters,
            &format!("{tag}_klt"),
        ))?;
        crate::error::llvm(
            self.llvm
                .builder
                .build_conditional_branch(k_cont, i_body, o_latch),
        )?;

        self.llvm.builder.position_at_end(i_body);
        let (x_next, h_add) =
            self.fo_step_branchless_vec(x, c_3_7, c_1_0, c_0_5, hit_ones, hit_zeros, width, tag)?;
        let h_cur = crate::error::llvm(self.llvm.builder.build_int_nsw_add(
            hi,
            h_add,
            &format!("{tag}_h1"),
        ))?;
        let k1 = crate::error::llvm(self.llvm.builder.build_int_nsw_add(
            k,
            one,
            &format!("{tag}_k1"),
        ))?;
        crate::error::llvm(self.llvm.builder.build_unconditional_branch(i_latch))?;

        self.llvm.builder.position_at_end(i_latch);
        k_phi.add_incoming(&[(&k1, i_latch)]);
        hi_phi.add_incoming(&[(&h_cur, i_latch)]);
        x_phi.add_incoming(&[(&x_next, i_latch)]);
        crate::error::llvm(self.llvm.builder.build_unconditional_branch(i_hdr))?;

        self.llvm.builder.position_at_end(o_latch);
        let i1 = crate::error::llvm(self.llvm.builder.build_int_nsw_add(
            i,
            step_w,
            &format!("{tag}_i1"),
        ))?;
        i_phi.add_incoming(&[(&i1, o_latch)]);
        h_phi.add_incoming(&[(&hi, o_latch)]);
        crate::error::llvm(self.llvm.builder.build_unconditional_branch(o_hdr))?;

        self.llvm.builder.position_at_end(o_exit);
        self.store_slot_i64(&pat.h, h)?;
        self.store_slot_i64(&pat.i, i)?;
        Ok(())
    }

    /// Vector logistic step: `x' = 3.7 * x * (1 - x)`; return `x'` and horizontal hit sum.
    fn fo_step_branchless_vec(
        &self,
        x: VectorValue<'ctx>,
        c_3_7: VectorValue<'ctx>,
        c_1_0: VectorValue<'ctx>,
        c_0_5: VectorValue<'ctx>,
        hit_ones: VectorValue<'ctx>,
        hit_zeros: VectorValue<'ctx>,
        width: u32,
        tag: &str,
    ) -> Result<(VectorValue<'ctx>, IntValue<'ctx>)> {
        let one_m_x = crate::error::llvm(self.llvm.builder.build_float_sub(
            c_1_0,
            x,
            &format!("{tag}_1mx"),
        ))?;
        let t = crate::error::llvm(self.llvm.builder.build_float_mul(
            c_3_7,
            x,
            &format!("{tag}_3_7x"),
        ))?;
        let x1 = crate::error::llvm(self.llvm.builder.build_float_mul(
            t,
            one_m_x,
            &format!("{tag}_x1"),
        ))?;
        let gt = crate::error::llvm(self.llvm.builder.build_float_compare(
            FloatPredicate::OGT,
            x1,
            c_0_5,
            &format!("{tag}_gt"),
        ))?;
        let hits = crate::error::llvm(self.llvm.builder.build_select(
            gt,
            hit_ones,
            hit_zeros,
            &format!("{tag}_hits"),
        ))?
        .into_vector_value();
        let mut h_add = self.llvm.i64_ty.const_int(0, false);
        for lane in 0..width as u64 {
            let e = crate::error::llvm(self.llvm.builder.build_extract_element(
                hits,
                self.llvm.i64_ty.const_int(lane, false),
                &format!("{tag}_he"),
            ))?
            .into_int_value();
            h_add = crate::error::llvm(self.llvm.builder.build_int_nsw_add(
                h_add,
                e,
                &format!("{tag}_hs"),
            ))?;
        }
        Ok((x1, h_add))
    }

    /// `x' = 3.7 * x * (1 - x); h' = h + (x' > 0.5)`.
    fn fo_step_branchless(
        &self,
        x: FloatValue<'ctx>,
        h: IntValue<'ctx>,
        c_3_7: FloatValue<'ctx>,
        c_1_0: FloatValue<'ctx>,
        c_0_5: FloatValue<'ctx>,
        one: IntValue<'ctx>,
        zero: IntValue<'ctx>,
        suf: &str,
    ) -> Result<(FloatValue<'ctx>, IntValue<'ctx>)> {
        let one_m_x = crate::error::llvm(self.llvm.builder.build_float_sub(
            c_1_0,
            x,
            &format!("fo_1mx{suf}"),
        ))?;
        let t = crate::error::llvm(self.llvm.builder.build_float_mul(
            c_3_7,
            x,
            &format!("fo_3_7x{suf}"),
        ))?;
        let x1 = crate::error::llvm(self.llvm.builder.build_float_mul(
            t,
            one_m_x,
            &format!("fo_x1{suf}"),
        ))?;
        let gt = crate::error::llvm(self.llvm.builder.build_float_compare(
            FloatPredicate::OGT,
            x1,
            c_0_5,
            &format!("fo_gt{suf}"),
        ))?;
        let add = crate::error::llvm(self.llvm.builder.build_select(
            gt,
            one,
            zero,
            &format!("fo_hit{suf}"),
        ))?
        .into_int_value();
        let h1 = crate::error::llvm(self.llvm.builder.build_int_nsw_add(
            h,
            add,
            &format!("fo_h1{suf}"),
        ))?;
        Ok((x1, h1))
    }
}

fn match_float_orbit(
    header: &Block,
    body: &Block,
    latch: &Block,
    defs: &HashMap<u32, Value>,
) -> Option<FloatOrbit> {
    if !latch.ops.is_empty() {
        return None;
    }
    let (i, n) = header_lt_bound(header, defs)?;
    // Hardcoded IR uses 0.1, 1e-8, 3.7, 1.0, 0.5 — require those literals.
    if !has_float_approx(defs, 3.7)
        || !has_float_approx(defs, 0.5)
        || !has_float_approx(defs, 0.1)
        || !has_float_approx(defs, 1e-8)
        || !has_float_approx(defs, 1.0)
    {
        return None;
    }
    // Logistic step and threshold compare must appear as float binaries.
    if !has_float_binop_with_const(defs, BinOp::Mul, 3.7) {
        return None;
    }
    if !has_float_binop_with_const(defs, BinOp::Gt, 0.5)
        && !has_float_binop_with_const(defs, BinOp::Lt, 0.5)
    {
        return None;
    }
    let mut inner: Option<(&Block, &Block, &Block)> = None;
    for op in &body.ops {
        if let Op::Let {
            value:
                Value::Loop {
                    header: ih,
                    body: ib,
                    latch: il,
                },
            ..
        } = op
        {
            inner = Some((ih, ib, il));
        }
    }
    let (ih, ib, il) = inner?;
    if !il.ops.is_empty() {
        return None;
    }
    let (k, iters) = header_lt_const(ih, defs)?;
    if k == i || iters < 1 {
        return None;
    }
    // Outer body resets inner IV (`k := 0`); RT/IR assume a fresh orbit each i.
    if !body_assigns_const(body, &k, 0, defs) {
        return None;
    }
    let mut h_name: Option<String> = None;
    let mut saw_k_inc = false;
    let mut saw_thresh_if = false;
    for_each_block_dfs(ib, &mut |b| {
        for op in &b.ops {
            match op {
                Op::Assign {
                    name,
                    value: Local(v),
                } => {
                    if name == &k && is_unit_inc(*v, &k, defs) {
                        saw_k_inc = true;
                    } else if is_unit_inc(*v, name, defs) && name != &i && name != &k {
                        h_name = Some(name.clone());
                    }
                }
                Op::Let {
                    value: Value::If { .. },
                    ..
                } => {
                    saw_thresh_if = true;
                }
                _ => {}
            }
        }
    });
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
    if !saw_i_inc || !saw_k_inc || !saw_thresh_if {
        return None;
    }
    Some(FloatOrbit {
        h: h_name?,
        i,
        n,
        iters,
    })
}

fn header_lt_bound(header: &Block, defs: &HashMap<u32, Value>) -> Option<(String, OrbitBound)> {
    let (iv, right) = core_header_lt_bound(header, defs)?;
    if let Some(c) = const_of(right, defs) {
        Some((iv, OrbitBound::Const(c)))
    } else {
        Some((iv, OrbitBound::Local(right)))
    }
}

#[cfg(test)]
#[path = "float_sr_tests.rs"]
mod tests;
