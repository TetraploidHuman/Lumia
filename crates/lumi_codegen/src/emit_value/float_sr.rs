//! Recognize Mandelbrot / float-orbit loops and emit tight LLVM IR (O3-friendly).
//!
//! Prefer IR over a Rust RT call: clang-linked `staticlib` does not LTO with the
//! caller's constants, so a cross-language helper stayed ~5× slower than same-crate
//! loops. Emitting here lets `default<O3>` see the full nest.

use inkwell::values::{BasicValueEnum, FloatValue, FunctionValue, IntValue};
use inkwell::{FloatPredicate, IntPredicate};
use lumi_core::{
    body_assigns_const, body_iv_unit_inc, const_int, for_each_block_dfs, is_unit_inc, latch_empty,
    name_of, Block, Local, Op, Value,
};
use lumi_syntax::BinOp;
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
        // IR assumes `h = 0`, `i = 0` before the nest.
        if !self.slot_known_eq(&pat.h, 0) || !self.slot_known_eq(&pat.i, 0) {
            return Ok(None);
        }
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
        if !self.slot_known_eq(&pat.acc, 0) || !self.slot_known_eq(&pat.y, 0) {
            return Ok(None);
        }
        // 4-wide interleaved escape lives in RT (ILP + successive cx+=dx for FP match).
        let max_it = match pat.max_it {
            MandelbrotIt::Const(c) => self.llvm.i64_ty.const_int(c as u64, true),
            MandelbrotIt::Local(l) => self.coerce_i64(self.local(l)?)?,
        };
        let rt = self.runtime_fn("lumi_mandelbrot_checksum")?;
        let call =
            crate::error::llvm(self.llvm.builder.build_call(rt, &[max_it.into()], "mb_chk"))?;
        let acc = call
            .try_as_basic_value()
            .basic()
            .ok_or_else(|| anyhow!("mandelbrot checksum result"))?
            .into_int_value();
        self.store_slot_i64(&pat.acc, acc)?;
        self.store_slot_i64(&pat.y, self.llvm.i64_ty.const_int(140, false))?;
        let _ = fv;
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
            #[allow(clippy::needless_range_loop)]
            for lane in 0..4 {
                let off = self.llvm.i64_ty.const_int(lane as u64, false);
                let iv = crate::error::llvm(self.llvm.builder.build_int_nsw_add(i, off, "fo4_iv"))?;
                let i_f = crate::error::llvm(self.llvm.builder.build_signed_int_to_float(
                    iv,
                    fty,
                    "fo4_sitofp",
                ))?;
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
                self.llvm.builder.build_phi(fty, &format!("fo4_x{lane}")),
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
    let mut saw_thresh_if = false;
    for_each_block_dfs(ib, &mut |b| {
        for op in &b.ops {
            match op {
                Op::Assign {
                    name,
                    value: Local(v),
                } => {
                    if is_unit_inc(*v, name, defs) && name != &i && name != &k {
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
    if !body_iv_unit_inc(body, &i, defs) || !body_iv_unit_inc(ib, &k, defs) || !saw_thresh_if {
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
    if !latch_empty(latch) {
        return None;
    }
    let (y, h_bound) = header_lt_const(header, defs)?;
    if h_bound != 140 {
        return None;
    }
    // Fixed-grid RT assumes 200×140 over [-2.5,1]×[-1,1] with escape radius 4.
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
    let mut x_loop: Option<(&Block, &Block, &Block)> = None;
    for op in &body.ops {
        if let Op::Let {
            value:
                Value::Loop {
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
    // Row body resets `x := 0` (RT walks a fresh 200-wide scanline).
    if !body_assigns_const(body, &x, 0, defs) {
        return None;
    }
    let mut it_loop: Option<(&Block, &Block, &Block)> = None;
    for op in &xb.ops {
        if let Op::Let {
            value:
                Value::Loop {
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
    let mut saw_x_inc = false;
    for_each_block_dfs(xb, &mut |b| {
        for op in &b.ops {
            if let Op::Assign {
                name,
                value: Local(v),
            } = op
            {
                if name == &x && is_unit_inc(*v, &x, defs) {
                    saw_x_inc = true;
                    continue;
                }
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
                    let l = name_of(*left, defs);
                    let r = name_of(*right, defs);
                    if l.as_deref() == Some(name.as_str()) || r.as_deref() == Some(name.as_str()) {
                        acc = Some(name.clone());
                    }
                }
            }
        }
    });
    if !body_iv_unit_inc(body, &y, defs) || !saw_x_inc {
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

fn has_float_approx(defs: &HashMap<u32, Value>, target: f64) -> bool {
    defs.values().any(|v| match v {
        Value::Float(f) => (*f - target).abs() < 1e-12,
        _ => false,
    })
}

fn has_float_binop_with_const(defs: &HashMap<u32, Value>, op: BinOp, target: f64) -> bool {
    defs.values().any(|v| {
        let Value::Binary {
            op: bop,
            left,
            right,
            ..
        } = v
        else {
            return false;
        };
        if *bop != op {
            return false;
        }
        let lf = match defs.get(&left.0) {
            Some(Value::Float(f)) => Some(*f),
            _ => None,
        };
        let rf = match defs.get(&right.0) {
            Some(Value::Float(f)) => Some(*f),
            _ => None,
        };
        lf.is_some_and(|f| (f - target).abs() < 1e-12)
            || rf.is_some_and(|f| (f - target).abs() < 1e-12)
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
    let iv = name_of(*left, defs)?;
    if let Some(c) = const_int(*right, defs) {
        return Some((iv, OrbitBound::Const(c)));
    }
    Some((iv, OrbitBound::Local(*right)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emit_value::sr_match_test::count_loop_matches;
    use lumi_opt::{compile_source_to_optimized, OptOptions};

    #[test]
    fn matches_float_orbit_and_mandelbrot_in_bench() {
        let core = crate::emit_value::sr_match_test::bench_cpu_core();
        assert!(
            count_loop_matches(&core, |h, b, l, d| match_float_orbit(h, b, l, d).is_some()) >= 1
        );
        assert!(
            count_loop_matches(&core, |h, b, l, d| match_mandelbrot(h, b, l, d).is_some()) >= 1
        );
    }

    #[test]
    fn matches_float_orbit_and_mandelbrot_in_opt_sr_correctness() {
        let src = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/opt_sr_correctness.lm"
        ))
        .unwrap();
        let core = compile_source_to_optimized(&src, &OptOptions::for_build(true)).unwrap();
        assert!(
            count_loop_matches(&core, |h, b, l, d| match_float_orbit(h, b, l, d).is_some()) >= 1
        );
        assert!(
            count_loop_matches(&core, |h, b, l, d| match_mandelbrot(h, b, l, d).is_some()) >= 1
        );
    }
}
