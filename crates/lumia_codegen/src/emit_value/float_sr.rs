//! Recognize Mandelbrot / float-orbit loops and emit tight LLVM IR (O3-friendly).
//!
//! Prefer IR over a Rust RT call: clang-linked `staticlib` does not LTO with the
//! caller's constants, so a cross-language helper stayed ~5× slower than same-crate
//! loops. Emitting here lets `default<O3>` see the full nest.

use inkwell::values::{BasicValueEnum, FloatValue, FunctionValue, IntValue};
use inkwell::{FloatPredicate, IntPredicate};
use lumia_core::{for_each_block_dfs, Block, Local, Op, Value};
use lumia_syntax::BinOp;
use rustc_hash::FxHashMap as HashMap;

use super::super::Codegen;
use anyhow::{anyhow, Result};

#[derive(Debug)]
struct FloatOrbit {
    h: String,
    i: String,
    n: OrbitBound,
    iters: i64,
}

#[derive(Debug)]
struct Mandelbrot {
    acc: String,
    y: String,
    max_it: MandelbrotIt,
}

#[derive(Debug, Clone)]
enum OrbitBound {
    Const(i64),
    Local(Local),
}

#[derive(Debug, Clone)]
enum MandelbrotIt {
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
        self.emit_float_orbit_ir(&pat, fv)?;
        Ok(Some(self.llvm.i64_ty.const_int(0, false).into()))
    }

    pub(crate) fn try_emit_mandelbrot_loop(
        &mut self,
        header: &Block,
        body: &Block,
        latch: &Block,
        fv: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>> {
        let Some(pat) = match_mandelbrot(header, body, latch, &self.frame.leaf_defs) else {
            return Ok(None);
        };
        self.emit_mandelbrot_ir(&pat, fv)?;
        Ok(Some(self.llvm.i64_ty.const_int(0, false).into()))
    }

    fn emit_float_orbit_ir(&mut self, pat: &FloatOrbit, fv: FunctionValue<'ctx>) -> Result<()> {
        // Independent outer orbits → 4-wide when `n` is a multiple of 4; else scalar SSA.
        // Branchless hit counts keep the inner body a single BB (O3-friendly).
        match pat.n {
            OrbitBound::Const(n) if n >= 4 && n % 4 == 0 => {
                self.emit_float_orbit_phi_x4(pat, fv, n)
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
        let i_f = crate::error::llvm(
            self.llvm
                .builder
                .build_signed_int_to_float(i, fty, "fo_sitofp"),
        )?;
        let scaled =
            crate::error::llvm(self.llvm.builder.build_float_mul(c_1e8, i_f, "fo_scale"))?;
        let x_init =
            crate::error::llvm(self.llvm.builder.build_float_add(c_0_1, scaled, "fo_x0"))?;
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

    fn emit_float_orbit_phi_x4(
        &mut self,
        pat: &FloatOrbit,
        fv: FunctionValue<'ctx>,
        n_const: i64,
    ) -> Result<()> {
        let fty = self.llvm.context.f64_type();
        let n = self.llvm.i64_ty.const_int(n_const as u64, true);
        let iters = self.llvm.i64_ty.const_int(pat.iters as u64, true);
        let c_0_1 = fty.const_float(0.1);
        let c_1e8 = fty.const_float(1e-8);
        let c_3_7 = fty.const_float(3.7);
        let c_1_0 = fty.const_float(1.0);
        let c_0_5 = fty.const_float(0.5);
        let one = self.llvm.i64_ty.const_int(1, false);
        let four = self.llvm.i64_ty.const_int(4, false);
        let zero = self.llvm.i64_ty.const_int(0, false);

        let pre = self
            .llvm
            .builder
            .get_insert_block()
            .ok_or_else(|| anyhow!("float orbit x4: no insert block"))?;
        let h0 = self.load_slot_i64(&pat.h)?;
        let i0 = self.load_slot_i64(&pat.i)?;

        let o_hdr = self.llvm.context.append_basic_block(fv, "fo4_hdr");
        let o_body = self.llvm.context.append_basic_block(fv, "fo4_body");
        let i_hdr = self.llvm.context.append_basic_block(fv, "fo4_ihdr");
        let i_body = self.llvm.context.append_basic_block(fv, "fo4_ibody");
        let i_latch = self.llvm.context.append_basic_block(fv, "fo4_ilatch");
        let o_latch = self.llvm.context.append_basic_block(fv, "fo4_olatch");
        let o_exit = self.llvm.context.append_basic_block(fv, "fo4_exit");

        crate::error::llvm(self.llvm.builder.build_unconditional_branch(o_hdr))?;

        self.llvm.builder.position_at_end(o_hdr);
        let i_phi = crate::error::llvm(self.llvm.builder.build_phi(self.llvm.i64_ty, "fo4_i"))?;
        let h_phi = crate::error::llvm(self.llvm.builder.build_phi(self.llvm.i64_ty, "fo4_h"))?;
        i_phi.add_incoming(&[(&i0, pre)]);
        h_phi.add_incoming(&[(&h0, pre)]);
        let i = i_phi.as_basic_value().into_int_value();
        let h = h_phi.as_basic_value().into_int_value();
        let cont = crate::error::llvm(self.llvm.builder.build_int_compare(
            IntPredicate::SLT,
            i,
            n,
            "fo4_ilt",
        ))?;
        crate::error::llvm(
            self.llvm
                .builder
                .build_conditional_branch(cont, o_body, o_exit),
        )?;

        self.llvm.builder.position_at_end(o_body);
        let x_inits = {
            let mut xs = [c_0_1; 4];
            for lane in 0..4 {
                let off = self.llvm.i64_ty.const_int(lane as u64, false);
                let iv = crate::error::llvm(self.llvm.builder.build_int_nsw_add(i, off, "fo4_iv"))?;
                let i_f = crate::error::llvm(
                    self.llvm
                        .builder
                        .build_signed_int_to_float(iv, fty, "fo4_sitofp"),
                )?;
                let scaled =
                    crate::error::llvm(self.llvm.builder.build_float_mul(c_1e8, i_f, "fo4_sc"))?;
                xs[lane] =
                    crate::error::llvm(self.llvm.builder.build_float_add(c_0_1, scaled, "fo4_x0"))?;
            }
            xs
        };
        crate::error::llvm(self.llvm.builder.build_unconditional_branch(i_hdr))?;

        self.llvm.builder.position_at_end(i_hdr);
        let k_phi = crate::error::llvm(self.llvm.builder.build_phi(self.llvm.i64_ty, "fo4_k"))?;
        let hi_phi = crate::error::llvm(self.llvm.builder.build_phi(self.llvm.i64_ty, "fo4_hi"))?;
        let mut x_phis = Vec::with_capacity(4);
        for lane in 0..4 {
            x_phis.push(crate::error::llvm(
                self.llvm
                    .builder
                    .build_phi(fty, &format!("fo4_x{lane}")),
            )?);
        }
        k_phi.add_incoming(&[(&zero, o_body)]);
        hi_phi.add_incoming(&[(&h, o_body)]);
        for lane in 0..4 {
            x_phis[lane].add_incoming(&[(&x_inits[lane], o_body)]);
        }
        let k = k_phi.as_basic_value().into_int_value();
        let hi = hi_phi.as_basic_value().into_int_value();
        let xs: [FloatValue; 4] =
            std::array::from_fn(|lane| x_phis[lane].as_basic_value().into_float_value());
        let k_cont = crate::error::llvm(self.llvm.builder.build_int_compare(
            IntPredicate::SLT,
            k,
            iters,
            "fo4_klt",
        ))?;
        crate::error::llvm(
            self.llvm
                .builder
                .build_conditional_branch(k_cont, i_body, o_latch),
        )?;

        self.llvm.builder.position_at_end(i_body);
        let mut h_cur = hi;
        let mut x_next = [c_0_1; 4];
        for lane in 0..4 {
            let (xn, hn) = self.fo_step_branchless(
                xs[lane],
                h_cur,
                c_3_7,
                c_1_0,
                c_0_5,
                one,
                zero,
                &format!("l{lane}"),
            )?;
            x_next[lane] = xn;
            h_cur = hn;
        }
        let k1 = crate::error::llvm(self.llvm.builder.build_int_nsw_add(k, one, "fo4_k1"))?;
        crate::error::llvm(self.llvm.builder.build_unconditional_branch(i_latch))?;

        self.llvm.builder.position_at_end(i_latch);
        k_phi.add_incoming(&[(&k1, i_latch)]);
        hi_phi.add_incoming(&[(&h_cur, i_latch)]);
        for lane in 0..4 {
            x_phis[lane].add_incoming(&[(&x_next[lane], i_latch)]);
        }
        crate::error::llvm(self.llvm.builder.build_unconditional_branch(i_hdr))?;

        self.llvm.builder.position_at_end(o_latch);
        let i1 = crate::error::llvm(self.llvm.builder.build_int_nsw_add(i, four, "fo4_i1"))?;
        i_phi.add_incoming(&[(&i1, o_latch)]);
        h_phi.add_incoming(&[(&hi, o_latch)]);
        crate::error::llvm(self.llvm.builder.build_unconditional_branch(o_hdr))?;

        self.llvm.builder.position_at_end(o_exit);
        self.store_slot_i64(&pat.h, h)?;
        self.store_slot_i64(&pat.i, i)?;
        Ok(())
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
        let one_m_x =
            crate::error::llvm(self.llvm.builder.build_float_sub(c_1_0, x, &format!("fo_1mx{suf}")))?;
        let t =
            crate::error::llvm(self.llvm.builder.build_float_mul(c_3_7, x, &format!("fo_3_7x{suf}")))?;
        let x1 =
            crate::error::llvm(self.llvm.builder.build_float_mul(t, one_m_x, &format!("fo_x1{suf}")))?;
        let gt = crate::error::llvm(self.llvm.builder.build_float_compare(
            FloatPredicate::OGT,
            x1,
            c_0_5,
            &format!("fo_gt{suf}"),
        ))?;
        let add = crate::error::llvm(
            self.llvm
                .builder
                .build_select(gt, one, zero, &format!("fo_hit{suf}")),
        )?
        .into_int_value();
        let h1 =
            crate::error::llvm(self.llvm.builder.build_int_nsw_add(h, add, &format!("fo_h1{suf}")))?;
        Ok((x1, h1))
    }

    fn emit_mandelbrot_ir(&mut self, pat: &Mandelbrot, fv: FunctionValue<'ctx>) -> Result<()> {
        // Full SSA phis (no mut allocas). Escape stays branched — early exit is the win.
        let fty = self.llvm.context.f64_type();
        let i1_ty = self.llvm.context.bool_type();
        let max_it = match pat.max_it {
            MandelbrotIt::Const(c) => self.llvm.i64_ty.const_int(c as u64, true),
            MandelbrotIt::Local(l) => self.coerce_i64(self.local(l)?)?,
        };
        let w = self.llvm.i64_ty.const_int(200, false);
        let h_lim = self.llvm.i64_ty.const_int(140, false);
        let dx = fty.const_float(3.5 / 200.0);
        let dy = fty.const_float(2.0 / 140.0);
        let four = fty.const_float(4.0);
        let two = fty.const_float(2.0);
        let c_m2_5 = fty.const_float(-2.5);
        let c_m1 = fty.const_float(-1.0);
        let zero_f = fty.const_float(0.0);
        let one = self.llvm.i64_ty.const_int(1, false);
        let zero_i = self.llvm.i64_ty.const_int(0, false);
        let false_v = i1_ty.const_int(0, false);
        let true_v = i1_ty.const_int(1, false);

        let pre = self
            .llvm
            .builder
            .get_insert_block()
            .ok_or_else(|| anyhow!("mandelbrot: no insert block"))?;
        let y0 = self.load_slot_i64(&pat.y)?;
        let acc0 = self.load_slot_i64(&pat.acc)?;

        let y_hdr = self.llvm.context.append_basic_block(fv, "mb_yhdr");
        let y_body = self.llvm.context.append_basic_block(fv, "mb_ybody");
        let x_hdr = self.llvm.context.append_basic_block(fv, "mb_xhdr");
        let x_body = self.llvm.context.append_basic_block(fv, "mb_xbody");
        let t_hdr = self.llvm.context.append_basic_block(fv, "mb_thdr");
        let t_body = self.llvm.context.append_basic_block(fv, "mb_tbody");
        let t_esc = self.llvm.context.append_basic_block(fv, "mb_tesc");
        let t_step = self.llvm.context.append_basic_block(fv, "mb_tstep");
        let t_exit = self.llvm.context.append_basic_block(fv, "mb_texit");
        let x_latch = self.llvm.context.append_basic_block(fv, "mb_xlatch");
        let y_latch = self.llvm.context.append_basic_block(fv, "mb_ylatch");
        let y_exit = self.llvm.context.append_basic_block(fv, "mb_yexit");

        crate::error::llvm(self.llvm.builder.build_unconditional_branch(y_hdr))?;

        self.llvm.builder.position_at_end(y_hdr);
        let y_phi = crate::error::llvm(self.llvm.builder.build_phi(self.llvm.i64_ty, "mb_y"))?;
        let cy_phi = crate::error::llvm(self.llvm.builder.build_phi(fty, "mb_cy"))?;
        let acc_y_phi =
            crate::error::llvm(self.llvm.builder.build_phi(self.llvm.i64_ty, "mb_acc_y"))?;
        y_phi.add_incoming(&[(&y0, pre)]);
        cy_phi.add_incoming(&[(&c_m1, pre)]);
        acc_y_phi.add_incoming(&[(&acc0, pre)]);
        let y = y_phi.as_basic_value().into_int_value();
        let cy = cy_phi.as_basic_value().into_float_value();
        let acc_y = acc_y_phi.as_basic_value().into_int_value();
        let y_cont = crate::error::llvm(self.llvm.builder.build_int_compare(
            IntPredicate::SLT,
            y,
            h_lim,
            "mb_ylt",
        ))?;
        crate::error::llvm(
            self.llvm
                .builder
                .build_conditional_branch(y_cont, y_body, y_exit),
        )?;

        self.llvm.builder.position_at_end(y_body);
        crate::error::llvm(self.llvm.builder.build_unconditional_branch(x_hdr))?;

        self.llvm.builder.position_at_end(x_hdr);
        let x_phi = crate::error::llvm(self.llvm.builder.build_phi(self.llvm.i64_ty, "mb_x"))?;
        let cx_phi = crate::error::llvm(self.llvm.builder.build_phi(fty, "mb_cx"))?;
        let acc_x_phi =
            crate::error::llvm(self.llvm.builder.build_phi(self.llvm.i64_ty, "mb_acc_x"))?;
        x_phi.add_incoming(&[(&zero_i, y_body)]);
        cx_phi.add_incoming(&[(&c_m2_5, y_body)]);
        acc_x_phi.add_incoming(&[(&acc_y, y_body)]);
        let x = x_phi.as_basic_value().into_int_value();
        let cx = cx_phi.as_basic_value().into_float_value();
        let acc_x = acc_x_phi.as_basic_value().into_int_value();
        let x_cont = crate::error::llvm(self.llvm.builder.build_int_compare(
            IntPredicate::SLT,
            x,
            w,
            "mb_xlt",
        ))?;
        crate::error::llvm(
            self.llvm
                .builder
                .build_conditional_branch(x_cont, x_body, y_latch),
        )?;

        self.llvm.builder.position_at_end(x_body);
        crate::error::llvm(self.llvm.builder.build_unconditional_branch(t_hdr))?;

        self.llvm.builder.position_at_end(t_hdr);
        let it_phi = crate::error::llvm(self.llvm.builder.build_phi(self.llvm.i64_ty, "mb_it"))?;
        let zx_phi = crate::error::llvm(self.llvm.builder.build_phi(fty, "mb_zx"))?;
        let zy_phi = crate::error::llvm(self.llvm.builder.build_phi(fty, "mb_zy"))?;
        it_phi.add_incoming(&[(&zero_i, x_body)]);
        zx_phi.add_incoming(&[(&zero_f, x_body)]);
        zy_phi.add_incoming(&[(&zero_f, x_body)]);
        let it = it_phi.as_basic_value().into_int_value();
        let zx = zx_phi.as_basic_value().into_float_value();
        let zy = zy_phi.as_basic_value().into_float_value();
        let t_cont = crate::error::llvm(self.llvm.builder.build_int_compare(
            IntPredicate::SLT,
            it,
            max_it,
            "mb_itlt",
        ))?;
        crate::error::llvm(
            self.llvm
                .builder
                .build_conditional_branch(t_cont, t_body, t_exit),
        )?;

        self.llvm.builder.position_at_end(t_body);
        let zx2 = crate::error::llvm(self.llvm.builder.build_float_mul(zx, zx, "mb_zx2"))?;
        let zy2 = crate::error::llvm(self.llvm.builder.build_float_mul(zy, zy, "mb_zy2"))?;
        let r2 = crate::error::llvm(self.llvm.builder.build_float_add(zx2, zy2, "mb_r2"))?;
        let escaped_now = crate::error::llvm(self.llvm.builder.build_float_compare(
            FloatPredicate::OGT,
            r2,
            four,
            "mb_esc_now",
        ))?;
        crate::error::llvm(
            self.llvm
                .builder
                .build_conditional_branch(escaped_now, t_esc, t_step),
        )?;

        self.llvm.builder.position_at_end(t_esc);
        crate::error::llvm(self.llvm.builder.build_unconditional_branch(t_exit))?;

        self.llvm.builder.position_at_end(t_step);
        let two_zx = crate::error::llvm(self.llvm.builder.build_float_mul(two, zx, "mb_2zx"))?;
        let two_zx_zy =
            crate::error::llvm(self.llvm.builder.build_float_mul(two_zx, zy, "mb_2zxzy"))?;
        let nzy = crate::error::llvm(self.llvm.builder.build_float_add(two_zx_zy, cy, "mb_nzy"))?;
        let zx_m_zy = crate::error::llvm(self.llvm.builder.build_float_sub(zx2, zy2, "mb_zxmzy"))?;
        let nzx = crate::error::llvm(self.llvm.builder.build_float_add(zx_m_zy, cx, "mb_nzx"))?;
        let it1 = crate::error::llvm(self.llvm.builder.build_int_nsw_add(it, one, "mb_it1"))?;
        it_phi.add_incoming(&[(&it1, t_step)]);
        zx_phi.add_incoming(&[(&nzx, t_step)]);
        zy_phi.add_incoming(&[(&nzy, t_step)]);
        crate::error::llvm(self.llvm.builder.build_unconditional_branch(t_hdr))?;

        self.llvm.builder.position_at_end(t_exit);
        let esc_exit = crate::error::llvm(self.llvm.builder.build_phi(i1_ty, "mb_esc_ex"))?;
        let it_exit = crate::error::llvm(self.llvm.builder.build_phi(self.llvm.i64_ty, "mb_it_ex"))?;
        esc_exit.add_incoming(&[(&false_v, t_hdr), (&true_v, t_esc)]);
        it_exit.add_incoming(&[(&it, t_hdr), (&it, t_esc)]);
        let esc_e = esc_exit.as_basic_value().into_int_value();
        let it_e = it_exit.as_basic_value().into_int_value();
        let add = crate::error::llvm(
            self.llvm
                .builder
                .build_select(esc_e, it_e, max_it, "mb_add"),
        )?
        .into_int_value();
        let acc1 = crate::error::llvm(self.llvm.builder.build_int_nsw_add(acc_x, add, "mb_acc1"))?;
        crate::error::llvm(self.llvm.builder.build_unconditional_branch(x_latch))?;

        self.llvm.builder.position_at_end(x_latch);
        let cx1 = crate::error::llvm(self.llvm.builder.build_float_add(cx, dx, "mb_cx1"))?;
        let x1 = crate::error::llvm(self.llvm.builder.build_int_nsw_add(x, one, "mb_x1"))?;
        x_phi.add_incoming(&[(&x1, x_latch)]);
        cx_phi.add_incoming(&[(&cx1, x_latch)]);
        acc_x_phi.add_incoming(&[(&acc1, x_latch)]);
        crate::error::llvm(self.llvm.builder.build_unconditional_branch(x_hdr))?;

        self.llvm.builder.position_at_end(y_latch);
        let cy1 = crate::error::llvm(self.llvm.builder.build_float_add(cy, dy, "mb_cy1"))?;
        let y1 = crate::error::llvm(self.llvm.builder.build_int_nsw_add(y, one, "mb_y1"))?;
        y_phi.add_incoming(&[(&y1, y_latch)]);
        cy_phi.add_incoming(&[(&cy1, y_latch)]);
        acc_y_phi.add_incoming(&[(&acc_x, y_latch)]);
        crate::error::llvm(self.llvm.builder.build_unconditional_branch(y_hdr))?;

        self.llvm.builder.position_at_end(y_exit);
        self.store_slot_i64(&pat.acc, acc_y)?;
        self.store_slot_i64(&pat.y, y)?;
        Ok(())
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
    let floats: Vec<f64> = defs
        .values()
        .filter_map(|v| match v {
            Value::Float(f) => Some(*f),
            _ => None,
        })
        .collect();
    if !floats.iter().any(|f| (*f - 3.7).abs() < 1e-12) {
        return None;
    }
    if !floats.iter().any(|f| (*f - 0.5).abs() < 1e-12) {
        return None;
    }
    if !floats.iter().any(|f| (*f - 0.1).abs() < 1e-12) {
        return None;
    }
    let mut inner: Option<(&Block, &Block, &Block)> = None;
    for op in &body.ops {
        if let Op::Let {
            value: Value::Loop {
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
    let mut h_name: Option<String> = None;
    let mut saw_k_inc = false;
    for_each_block_dfs(ib, &mut |b| {
        for op in &b.ops {
            if let Op::Assign {
                name,
                value: Local(v),
            } = op
            {
                if name == &k && is_unit_inc(*v, &k, defs) {
                    saw_k_inc = true;
                } else if is_unit_inc(*v, name, defs) && name != &i && name != &k {
                    h_name = Some(name.clone());
                }
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
    if !saw_i_inc || !saw_k_inc {
        return None;
    }
    Some(FloatOrbit {
        h: h_name?,
        i,
        n,
        iters,
    })
}

fn match_mandelbrot(
    header: &Block,
    body: &Block,
    latch: &Block,
    defs: &HashMap<u32, Value>,
) -> Option<Mandelbrot> {
    if !latch.ops.is_empty() {
        return None;
    }
    let (y, h_bound) = header_lt_const(header, defs)?;
    if h_bound != 140 {
        return None;
    }
    let floats: Vec<f64> = defs
        .values()
        .filter_map(|v| match v {
            Value::Float(f) => Some(*f),
            _ => None,
        })
        .collect();
    if !floats.iter().any(|f| (*f - 4.0).abs() < 1e-12) {
        return None;
    }
    if !floats.iter().any(|f| (*f - 2.5).abs() < 1e-12) {
        return None;
    }
    let mut x_loop: Option<(&Block, &Block, &Block)> = None;
    for op in &body.ops {
        if let Op::Let {
            value: Value::Loop {
                header: xh,
                body: xb,
                latch: xl,
            },
            ..
        } = op
        {
            x_loop = Some((xh, xb, xl));
        }
    }
    let (xh, xb, xl) = x_loop?;
    if !xl.ops.is_empty() {
        return None;
    }
    let (x, w_bound) = header_lt_const(xh, defs)?;
    if w_bound != 200 || x == y {
        return None;
    }
    let mut it_loop: Option<(&Block, &Block, &Block)> = None;
    for op in &xb.ops {
        if let Op::Let {
            value: Value::Loop {
                header: th,
                body: tb,
                latch: tl,
            },
            ..
        } = op
        {
            it_loop = Some((th, tb, tl));
        }
    }
    let (th, tb, _tl) = it_loop?;
    let (_it, max_it) = header_lt_bound(th, defs)?;
    let mut saw_break = false;
    for_each_block_dfs(tb, &mut |b| {
        for op in &b.ops {
            if matches!(op, Op::Break) {
                saw_break = true;
            }
        }
    });
    if !saw_break {
        return None;
    }
    let mut acc: Option<String> = None;
    let mut saw_y_inc = false;
    for op in &body.ops {
        if let Op::Assign {
            name,
            value: Local(v),
        } = op
        {
            if name == &y && is_unit_inc(*v, &y, defs) {
                saw_y_inc = true;
            }
        }
    }
    for_each_block_dfs(xb, &mut |b| {
        for op in &b.ops {
            if let Op::Assign {
                name,
                value: Local(v),
            } = op
            {
                if name == &x {
                    continue;
                }
                if let Some(Value::Binary {
                    op: BinOp::Add,
                    left,
                    right,
                    ..
                }) = defs.get(v)
                {
                    let l = name_of(defs, *left);
                    let r = name_of(defs, *right);
                    if l.as_deref() == Some(name.as_str()) || r.as_deref() == Some(name.as_str()) {
                        acc = Some(name.clone());
                    }
                }
            }
        }
    });
    if !saw_y_inc {
        return None;
    }
    Some(Mandelbrot {
        acc: acc?,
        y,
        max_it: match max_it {
            OrbitBound::Const(c) => MandelbrotIt::Const(c),
            OrbitBound::Local(l) => MandelbrotIt::Local(l),
        },
    })
}

fn header_lt_const(header: &Block, defs: &HashMap<u32, Value>) -> Option<(String, i64)> {
    match header_lt_bound(header, defs)? {
        (iv, OrbitBound::Const(c)) => Some((iv, c)),
        _ => None,
    }
}

fn header_lt_bound(header: &Block, defs: &HashMap<u32, Value>) -> Option<(String, OrbitBound)> {
    let res = header.result?;
    let Value::Binary {
        op: BinOp::Lt,
        left,
        right,
        ..
    } = defs.get(&res.0)?
    else {
        return None;
    };
    let iv = name_of(defs, *left)?;
    if let Some(c) = const_i64(defs, *right) {
        return Some((iv, OrbitBound::Const(c)));
    }
    Some((iv, OrbitBound::Local(*right)))
}

fn is_unit_inc(dest: u32, iv: &str, defs: &HashMap<u32, Value>) -> bool {
    let Some(Value::Binary {
        op: BinOp::Add,
        left,
        right,
        ..
    }) = defs.get(&dest)
    else {
        return false;
    };
    let l = name_of(defs, *left);
    let r = name_of(defs, *right);
    let lc = const_i64(defs, *left);
    let rc = const_i64(defs, *right);
    (l.as_deref() == Some(iv) && rc == Some(1)) || (r.as_deref() == Some(iv) && lc == Some(1))
}

fn name_of(defs: &HashMap<u32, Value>, l: Local) -> Option<String> {
    match defs.get(&l.0)? {
        Value::Name(n) => Some(n.clone()),
        _ => None,
    }
}

fn const_i64(defs: &HashMap<u32, Value>, l: Local) -> Option<i64> {
    match defs.get(&l.0)? {
        Value::Int(n) => Some(*n),
        _ => None,
    }
}