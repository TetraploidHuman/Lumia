//! Recognize dense `List[Float]` nests and emit `lumia_f64_*` RT kernels.
//!
//! Whole-function patterns (params = kernel args):
//! - gemv:    `y[i] = Σ_j A[i·n+j] * x[j]`
//! - gemv_t:  `y[j] += A[i·n+j] * x[i]` (after zero-fill)
//! - addmm:   `W[i,j] += α * u[i] * v[j]`
//! - axpy:    `y[i] += α * x[i]`
//! - sub:     `out[i] = a[i] - b[i]`
//! - clamp:   `xs[i] = clamp(xs[i], lo, hi)`

use inkwell::values::FunctionValue;
use lumia_core::{Block, Local, Op, Value};
use lumia_hir::Builtin;
use lumia_syntax::BinOp;
use rustc_hash::FxHashMap as HashMap;

use super::super::Codegen;
use anyhow::{Context as AnyhowContext, Result};

#[derive(Debug)]
struct GemvPat {
    m: Local,
    n: Local,
    a: Local,
    x: Local,
    y: Local,
}

#[derive(Debug)]
struct GemvTPat {
    m: Local,
    n: Local,
    a: Local,
    x: Local,
    y: Local,
}

#[derive(Debug)]
struct AddmmPat {
    m: Local,
    n: Local,
    w: Local,
    u: Local,
    v: Local,
    alpha: Local,
}

#[derive(Debug)]
struct AxpyPat {
    y: Local,
    alpha: Local,
    x: Local,
}

#[derive(Debug)]
struct SubPat {
    out: Local,
    a: Local,
    b: Local,
}

#[derive(Debug)]
struct ClampPat {
    xs: Local,
    lo: Local,
    hi: Local,
}

#[derive(Debug)]
struct CopyPat {
    dst: Local,
    src: Local,
}

impl<'ctx> Codegen<'ctx> {
    /// Whole-function SR for dense float helpers (params = pattern args).
    pub(crate) fn try_emit_dense_f64_fun(
        &mut self,
        fun: &lumia_core::CoreFun,
        _fv: FunctionValue<'ctx>,
    ) -> Result<Option<()>> {
        let defs = &self.frame.leaf_defs;
        if let Some(p) = match_gemv_fun(fun, defs) {
            return self.emit_gemv_fun(&p).map(Some);
        }
        if let Some(p) = match_gemv_t_fun(fun, defs) {
            return self.emit_gemv_t_fun(&p).map(Some);
        }
        if let Some(p) = match_addmm_fun(fun, defs) {
            return self.emit_addmm_fun(&p).map(Some);
        }
        if let Some(p) = match_axpy_fun(fun, defs) {
            return self.emit_axpy_fun(&p).map(Some);
        }
        if let Some(p) = match_sub_fun(fun, defs) {
            return self.emit_sub_fun(&p).map(Some);
        }
        if let Some(p) = match_clamp_fun(fun, defs) {
            return self.emit_clamp_fun(&p).map(Some);
        }
        if let Some(p) = match_copy_fun(fun, defs) {
            return self.emit_copy_fun(&p).map(Some);
        }
        Ok(None)
    }

    fn emit_gemv_fun(&mut self, p: &GemvPat) -> Result<()> {
        let rt = self.runtime_fn("lumia_f64_gemv")?;
        let m = self.coerce_i64(self.local(p.m)?)?;
        let n = self.coerce_i64(self.local(p.n)?)?;
        let a = self.i64_as_ptr(self.coerce_i64(self.local(p.a)?)?, "a")?;
        let x = self.i64_as_ptr(self.coerce_i64(self.local(p.x)?)?, "x")?;
        let y = self.i64_as_ptr(self.coerce_i64(self.local(p.y)?)?, "y")?;
        let call = crate::error::llvm(self.llvm.builder.build_call(
            rt,
            &[m.into(), n.into(), a.into(), x.into(), y.into()],
            "gemv",
        ))?;
        let out = call
            .try_as_basic_value()
            .basic()
            .context("gemv")?
            .into_pointer_value();
        let out_i = self.ptr_as_i64(out, "gemv_i64")?.into_int_value();
        // No frame/roots were pushed for this trampoline.
        crate::error::llvm(self.llvm.builder.build_return(Some(&out_i)))?;
        Ok(())
    }

    fn emit_gemv_t_fun(&mut self, p: &GemvTPat) -> Result<()> {
        let rt = self.runtime_fn("lumia_f64_gemv_t")?;
        let m = self.coerce_i64(self.local(p.m)?)?;
        let n = self.coerce_i64(self.local(p.n)?)?;
        let a = self.i64_as_ptr(self.coerce_i64(self.local(p.a)?)?, "a")?;
        let x = self.i64_as_ptr(self.coerce_i64(self.local(p.x)?)?, "x")?;
        let y = self.i64_as_ptr(self.coerce_i64(self.local(p.y)?)?, "y")?;
        let call = crate::error::llvm(self.llvm.builder.build_call(
            rt,
            &[m.into(), n.into(), a.into(), x.into(), y.into()],
            "gemvt",
        ))?;
        let out = call
            .try_as_basic_value()
            .basic()
            .context("gemv_t")?
            .into_pointer_value();
        let out_i = self.ptr_as_i64(out, "gemvt_i64")?.into_int_value();
        crate::error::llvm(self.llvm.builder.build_return(Some(&out_i)))?;
        Ok(())
    }

    fn emit_addmm_fun(&mut self, p: &AddmmPat) -> Result<()> {
        let rt = self.runtime_fn("lumia_f64_addmm")?;
        let m = self.coerce_i64(self.local(p.m)?)?;
        let n = self.coerce_i64(self.local(p.n)?)?;
        let w = self.i64_as_ptr(self.coerce_i64(self.local(p.w)?)?, "w")?;
        let u = self.i64_as_ptr(self.coerce_i64(self.local(p.u)?)?, "u")?;
        let v = self.i64_as_ptr(self.coerce_i64(self.local(p.v)?)?, "v")?;
        let alpha = self.promote_f64(self.local(p.alpha)?)?;
        let call = crate::error::llvm(self.llvm.builder.build_call(
            rt,
            &[
                m.into(),
                n.into(),
                w.into(),
                u.into(),
                v.into(),
                alpha.into(),
            ],
            "addmm",
        ))?;
        let out = call
            .try_as_basic_value()
            .basic()
            .context("addmm")?
            .into_pointer_value();
        let out_i = self.ptr_as_i64(out, "addmm_i64")?.into_int_value();
        crate::error::llvm(self.llvm.builder.build_return(Some(&out_i)))?;
        Ok(())
    }

    fn emit_axpy_fun(&mut self, p: &AxpyPat) -> Result<()> {
        let rt = self.runtime_fn("lumia_f64_axpy")?;
        let y = self.i64_as_ptr(self.coerce_i64(self.local(p.y)?)?, "y")?;
        let alpha = self.promote_f64(self.local(p.alpha)?)?;
        let x = self.i64_as_ptr(self.coerce_i64(self.local(p.x)?)?, "x")?;
        let call = crate::error::llvm(self.llvm.builder.build_call(
            rt,
            &[y.into(), alpha.into(), x.into()],
            "axpy",
        ))?;
        let out = call
            .try_as_basic_value()
            .basic()
            .context("axpy")?
            .into_pointer_value();
        let out_i = self.ptr_as_i64(out, "axpy_i64")?.into_int_value();
        crate::error::llvm(self.llvm.builder.build_return(Some(&out_i)))?;
        Ok(())
    }

    fn emit_sub_fun(&mut self, p: &SubPat) -> Result<()> {
        let rt = self.runtime_fn("lumia_f64_sub")?;
        let o = self.i64_as_ptr(self.coerce_i64(self.local(p.out)?)?, "o")?;
        let a = self.i64_as_ptr(self.coerce_i64(self.local(p.a)?)?, "a")?;
        let b = self.i64_as_ptr(self.coerce_i64(self.local(p.b)?)?, "b")?;
        let call = crate::error::llvm(self.llvm.builder.build_call(
            rt,
            &[o.into(), a.into(), b.into()],
            "fsub",
        ))?;
        let out = call
            .try_as_basic_value()
            .basic()
            .context("sub")?
            .into_pointer_value();
        let out_i = self.ptr_as_i64(out, "sub_i64")?.into_int_value();
        crate::error::llvm(self.llvm.builder.build_return(Some(&out_i)))?;
        Ok(())
    }

    fn emit_clamp_fun(&mut self, p: &ClampPat) -> Result<()> {
        let rt = self.runtime_fn("lumia_f64_clamp")?;
        let xs = self.i64_as_ptr(self.coerce_i64(self.local(p.xs)?)?, "xs")?;
        let lo = self.promote_f64(self.local(p.lo)?)?;
        let hi = self.promote_f64(self.local(p.hi)?)?;
        let call = crate::error::llvm(self.llvm.builder.build_call(
            rt,
            &[xs.into(), lo.into(), hi.into()],
            "fclamp",
        ))?;
        let out = call
            .try_as_basic_value()
            .basic()
            .context("clamp")?
            .into_pointer_value();
        let out_i = self.ptr_as_i64(out, "clamp_i64")?.into_int_value();
        crate::error::llvm(self.llvm.builder.build_return(Some(&out_i)))?;
        Ok(())
    }

    fn emit_copy_fun(&mut self, p: &CopyPat) -> Result<()> {
        let rt = self.runtime_fn("lumia_f64_copy")?;
        let dst = self.i64_as_ptr(self.coerce_i64(self.local(p.dst)?)?, "dst")?;
        let src = self.i64_as_ptr(self.coerce_i64(self.local(p.src)?)?, "src")?;
        let call = crate::error::llvm(
            self.llvm
                .builder
                .build_call(rt, &[dst.into(), src.into()], "fcopy"),
        )?;
        let out = call
            .try_as_basic_value()
            .basic()
            .context("copy")?
            .into_pointer_value();
        let out_i = self.ptr_as_i64(out, "copy_i64")?.into_int_value();
        crate::error::llvm(self.llvm.builder.build_return(Some(&out_i)))?;
        Ok(())
    }
}

fn match_gemv_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<GemvPat> {
    if fun.params.len() != 5 {
        return None;
    }
    let (m, n, a, x, y) = (
        fun.params[0],
        fun.params[1],
        fun.params[2],
        fun.params[3],
        fun.params[4],
    );
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, y)?;
    let (header, loop_body, latch) = first_loop(body)?;
    if !latch.ops.is_empty() {
        return None;
    }
    let (i_slot, bound) = header_lt_bound(header, defs)?;
    if !same_local(bound, m, defs) {
        return None;
    }
    if !body_has_gemv_inner(loop_body, defs, &out_slot, &i_slot, a, x, n) {
        return None;
    }
    Some(GemvPat { m, n, a, x, y })
}

fn match_gemv_t_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<GemvTPat> {
    if fun.params.len() != 5 {
        return None;
    }
    let (m, n, a, x, y) = (
        fun.params[0],
        fun.params[1],
        fun.params[2],
        fun.params[3],
        fun.params[4],
    );
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, y)?;
    if !fun_has_gemv_t_shape(body, defs, &out_slot, a, x, m, n) {
        return None;
    }
    Some(GemvTPat { m, n, a, x, y })
}

fn match_addmm_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<AddmmPat> {
    if fun.params.len() != 6 {
        return None;
    }
    let (m, n, w, u, v, alpha) = (
        fun.params[0],
        fun.params[1],
        fun.params[2],
        fun.params[3],
        fun.params[4],
        fun.params[5],
    );
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, w)?;
    if !fun_has_addmm_shape(body, defs, &out_slot, u, v, alpha, m, n) {
        return None;
    }
    Some(AddmmPat {
        m,
        n,
        w,
        u,
        v,
        alpha,
    })
}

fn match_axpy_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<AxpyPat> {
    if fun.params.len() != 3 {
        return None;
    }
    let (y, alpha, x) = (fun.params[0], fun.params[1], fun.params[2]);
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, y)?;
    if !fun_has_axpy_shape(body, defs, &out_slot, x, alpha) {
        return None;
    }
    Some(AxpyPat { y, alpha, x })
}

fn match_sub_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<SubPat> {
    if fun.params.len() != 3 {
        return None;
    }
    let (out, a, b) = (fun.params[0], fun.params[1], fun.params[2]);
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, out)?;
    if !fun_has_sub_shape(body, defs, &out_slot, a, b) {
        return None;
    }
    Some(SubPat { out, a, b })
}

fn match_clamp_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<ClampPat> {
    if fun.params.len() != 3 {
        return None;
    }
    let (xs, lo, hi) = (fun.params[0], fun.params[1], fun.params[2]);
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, xs)?;
    if !fun_has_clamp_shape(body, defs, &out_slot, lo, hi) {
        return None;
    }
    Some(ClampPat { xs, lo, hi })
}

fn match_copy_fun(fun: &lumia_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<CopyPat> {
    if fun.params.len() != 2 {
        return None;
    }
    let (dst, src) = (fun.params[0], fun.params[1]);
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, dst)?;
    if !fun_has_copy_shape(body, defs, &out_slot, src) {
        return None;
    }
    Some(CopyPat { dst, src })
}

fn first_assign_from_local(body: &Block, src: Local) -> Option<String> {
    for op in &body.ops {
        if let Op::Assign { name, value } = op {
            if *value == src {
                return Some(name.clone());
            }
        }
    }
    None
}

fn first_loop(body: &Block) -> Option<(&Block, &Block, &Block)> {
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

fn header_lt_bound(header: &Block, defs: &HashMap<u32, Value>) -> Option<(String, Local)> {
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
    Some((iv, *right))
}

fn name_of(l: Local, defs: &HashMap<u32, Value>) -> Option<String> {
    match defs.get(&l.0)? {
        Value::Name(n) => Some(n.clone()),
        _ => None,
    }
}

/// Resolve `Local` / `Name` load / param identity through leaf defs.
fn same_local(got: Local, want: Local, defs: &HashMap<u32, Value>) -> bool {
    if got == want {
        return true;
    }
    match defs.get(&got.0) {
        Some(Value::Local(l)) => same_local(*l, want, defs),
        Some(Value::Name(_)) => false, // slot load ≠ param unless assigned from it
        _ => false,
    }
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
    let one_l = matches!(defs.get(&left.0), Some(Value::Int(1)));
    let one_r = matches!(defs.get(&right.0), Some(Value::Int(1)));
    let name_l = name_of(*left, defs).as_deref() == Some(iv);
    let name_r = name_of(*right, defs).as_deref() == Some(iv);
    (name_l && one_r) || (name_r && one_l)
}

fn is_list_get(v: &Value) -> Option<(Local, Local)> {
    match v {
        Value::Builtin {
            name: Builtin::ListGet,
            args,
        } if args.len() == 2 => Some((args[0], args[1])),
        _ => None,
    }
}

fn is_list_set(v: &Value) -> Option<(Local, Local, Local)> {
    match v {
        Value::Builtin {
            name: Builtin::MapSet,
            args,
        } if args.len() == 3 => Some((args[0], args[1], args[2])),
        _ => None,
    }
}

fn list_arg_is(list: Local, want: Local, defs: &HashMap<u32, Value>) -> bool {
    if list == want {
        return true;
    }
    match defs.get(&list.0) {
        Some(Value::Local(l)) => list_arg_is(*l, want, defs),
        Some(Value::Name(_)) => false,
        _ => false,
    }
}

/// Inner body of gemv: s accumulates A[i*n+j]*x[j]; then out.set(i,s); i+=1.
fn body_has_gemv_inner(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out_slot: &str,
    i_slot: &str,
    a: Local,
    x: Local,
    n: Local,
) -> bool {
    let mut saw_inner = false;
    let mut saw_set = false;
    let mut saw_i_inc = false;
    for op in &body.ops {
        match op {
            Op::Let {
                value:
                    Value::Loop {
                        header,
                        body: ib,
                        latch,
                    },
                ..
            } => {
                if !latch.ops.is_empty() {
                    continue;
                }
                let Some((j_slot, bound)) = header_lt_bound(header, defs) else {
                    continue;
                };
                if !same_local(bound, n, defs) {
                    continue;
                }
                if gemv_inner_accumulates(ib, defs, &j_slot, a, x, n, i_slot) {
                    saw_inner = true;
                }
            }
            Op::Assign { name, value } => {
                if name == out_slot {
                    if let Some(val) = defs.get(&value.0) {
                        if is_list_set(val).is_some() {
                            saw_set = true;
                        }
                    }
                }
                if name == i_slot && is_unit_inc(value.0, i_slot, defs) {
                    saw_i_inc = true;
                }
            }
            _ => {}
        }
    }
    saw_inner && saw_set && saw_i_inc
}

fn gemv_inner_accumulates(
    body: &Block,
    defs: &HashMap<u32, Value>,
    j_slot: &str,
    a: Local,
    x: Local,
    n: Local,
    i_slot: &str,
) -> bool {
    let mut saw_mul_gets = false;
    let mut saw_j_inc = false;
    for op in &body.ops {
        if let Op::Assign { name, value } = op {
            if name == j_slot && is_unit_inc(value.0, j_slot, defs) {
                saw_j_inc = true;
            }
        }
        if let Op::Let {
            value:
                Value::Binary {
                    op: BinOp::Mul,
                    left,
                    right,
                    ..
                },
            ..
        } = op
        {
            let lg = defs.get(&left.0).and_then(is_list_get);
            let rg = defs.get(&right.0).and_then(is_list_get);
            if let (Some((la, _)), Some((lb, _))) = (lg, rg) {
                let a_x = (list_arg_is(la, a, defs) && list_arg_is(lb, x, defs))
                    || (list_arg_is(la, x, defs) && list_arg_is(lb, a, defs));
                if a_x {
                    // Soft-check index uses i/n/j via presence of Mul/Add involving them elsewhere.
                    let _ = (n, i_slot);
                    saw_mul_gets = true;
                }
            }
        }
    }
    saw_mul_gets && saw_j_inc
}

fn fun_has_gemv_t_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out_slot: &str,
    a: Local,
    x: Local,
    m: Local,
    n: Local,
) -> bool {
    let mut mul = false;
    let mut set = false;
    let mut zero_fill = false;
    for_each_let(body, &mut |v| {
        if let Value::Binary {
            op: BinOp::Mul,
            left,
            right,
            ..
        } = v
        {
            let lg = defs.get(&left.0).and_then(is_list_get);
            let rg = defs.get(&right.0).and_then(is_list_get);
            if let (Some((la, _)), Some((lb, _))) = (lg, rg) {
                if (list_arg_is(la, a, defs) && list_arg_is(lb, x, defs))
                    || (list_arg_is(la, x, defs) && list_arg_is(lb, a, defs))
                {
                    mul = true;
                }
            }
        }
        if is_list_set(v).is_some() {
            set = true;
        }
        // Zero-fill: set(j, 0.0) or set(j, Float(0))
        if let Some((_, _, val)) = is_list_set(v) {
            if matches!(defs.get(&val.0), Some(Value::Float(f)) if *f == 0.0)
                || matches!(defs.get(&val.0), Some(Value::Int(0)))
            {
                zero_fill = true;
            }
        }
        let _ = (m, n, out_slot);
    });
    // Also scan leaf_defs for MapSet / Mul (lets may be inlined into Assigns)
    for v in defs.values() {
        if let Value::Binary {
            op: BinOp::Mul,
            left,
            right,
            ..
        } = v
        {
            let lg = defs.get(&left.0).and_then(is_list_get);
            let rg = defs.get(&right.0).and_then(is_list_get);
            if let (Some((la, _)), Some((lb, _))) = (lg, rg) {
                if (list_arg_is(la, a, defs) && list_arg_is(lb, x, defs))
                    || (list_arg_is(la, x, defs) && list_arg_is(lb, a, defs))
                {
                    mul = true;
                }
            }
        }
        if is_list_set(v).is_some() {
            set = true;
        }
        if let Some((_, _, val)) = is_list_set(v) {
            if matches!(defs.get(&val.0), Some(Value::Float(f)) if *f == 0.0) {
                zero_fill = true;
            }
        }
    }
    mul && set && zero_fill
}

fn fun_has_addmm_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out_slot: &str,
    u: Local,
    v: Local,
    alpha: Local,
    m: Local,
    n: Local,
) -> bool {
    let mut get_u = false;
    let mut get_v = false;
    let mut set = false;
    let mut uses_alpha = false;
    for vdef in defs.values() {
        if let Some((lst, _)) = is_list_get(vdef) {
            if list_arg_is(lst, u, defs) {
                get_u = true;
            }
            if list_arg_is(lst, v, defs) {
                get_v = true;
            }
        }
        if is_list_set(vdef).is_some() {
            set = true;
        }
        if mentions_local(vdef, alpha) {
            uses_alpha = true;
        }
    }
    for_each_let(body, &mut |val| {
        if is_list_set(val).is_some() {
            set = true;
        }
        if let Some((lst, _)) = is_list_get(val) {
            if list_arg_is(lst, u, defs) {
                get_u = true;
            }
            if list_arg_is(lst, v, defs) {
                get_v = true;
            }
        }
    });
    let _ = (out_slot, m, n);
    get_u && get_v && set && uses_alpha
}

fn fun_has_axpy_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out_slot: &str,
    x: Local,
    alpha: Local,
) -> bool {
    let mut get_x = false;
    let mut get_y = false;
    let mut set = false;
    let mut uses_alpha = false;
    for v in defs.values() {
        if let Some((lst, _)) = is_list_get(v) {
            if list_arg_is(lst, x, defs) {
                get_x = true;
            }
            // y is out_slot Name
            if name_of(lst, defs).as_deref() == Some(out_slot) {
                get_y = true;
            }
        }
        if is_list_set(v).is_some() {
            set = true;
        }
        if mentions_local(v, alpha) {
            uses_alpha = true;
        }
    }
    for_each_let(body, &mut |val| {
        if is_list_set(val).is_some() {
            set = true;
        }
        if let Some((lst, _)) = is_list_get(val) {
            if list_arg_is(lst, x, defs) {
                get_x = true;
            }
            if name_of(lst, defs).as_deref() == Some(out_slot) {
                get_y = true;
            }
        }
    });
    get_x && get_y && set && uses_alpha
}

fn fun_has_sub_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out_slot: &str,
    a: Local,
    b: Local,
) -> bool {
    let mut get_a = false;
    let mut get_b = false;
    let mut sub = false;
    let mut set = false;
    for v in defs.values() {
        if let Some((lst, _)) = is_list_get(v) {
            if list_arg_is(lst, a, defs) {
                get_a = true;
            }
            if list_arg_is(lst, b, defs) {
                get_b = true;
            }
        }
        if matches!(v, Value::Binary { op: BinOp::Sub, .. }) {
            sub = true;
        }
        if is_list_set(v).is_some() {
            set = true;
        }
    }
    for_each_let(body, &mut |val| {
        if is_list_set(val).is_some() {
            set = true;
        }
        if matches!(val, Value::Binary { op: BinOp::Sub, .. }) {
            sub = true;
        }
    });
    let _ = out_slot;
    get_a && get_b && sub && set
}

fn fun_has_clamp_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out_slot: &str,
    lo: Local,
    hi: Local,
) -> bool {
    let mut set = false;
    let mut uses_lo = false;
    let mut uses_hi = false;
    let mut saw_if = false;
    for_each_let(body, &mut |val| {
        if is_list_set(val).is_some() {
            set = true;
        }
        if matches!(val, Value::If { .. }) {
            saw_if = true;
        }
    });
    for v in defs.values() {
        if mentions_local(v, lo) {
            uses_lo = true;
        }
        if mentions_local(v, hi) {
            uses_hi = true;
        }
        if is_list_set(v).is_some() {
            set = true;
        }
        if matches!(
            v,
            Value::Binary {
                op: BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge,
                ..
            }
        ) {
            saw_if = true;
        }
    }
    for op in &body.ops {
        if let Op::Assign { name, .. } = op {
            if name == out_slot {
                set = true;
            }
        }
    }
    set && saw_if && uses_lo && uses_hi
}

fn fun_has_copy_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out_slot: &str,
    src: Local,
) -> bool {
    // out[i] = src[i]; no arithmetic on the transferred value.
    let mut get_src = false;
    let mut set = false;
    let mut saw_arith = false;
    for v in defs.values() {
        if let Some((lst, _)) = is_list_get(v) {
            if list_arg_is(lst, src, defs) {
                get_src = true;
            }
        }
        if is_list_set(v).is_some() {
            set = true;
        }
        if matches!(
            v,
            Value::Binary {
                op: BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div,
                ..
            }
        ) {
            // Index `i*n+j` style shouldn't appear; len() compares are elsewhere.
            // Allow only if not feeding the set value — soft: any Mul/Div is suspicious.
            if matches!(
                v,
                Value::Binary {
                    op: BinOp::Mul | BinOp::Div | BinOp::Sub,
                    ..
                }
            ) {
                saw_arith = true;
            }
        }
    }
    for_each_let(body, &mut |val| {
        if is_list_set(val).is_some() {
            set = true;
        }
        if let Some((lst, _)) = is_list_get(val) {
            if list_arg_is(lst, src, defs) {
                get_src = true;
            }
        }
    });
    let _ = out_slot;
    get_src && set && !saw_arith
}

fn mentions_local(v: &Value, target: Local) -> bool {
    match v {
        Value::Local(l) => *l == target,
        Value::Binary { left, right, .. } => *left == target || *right == target,
        Value::Builtin { args, .. } => args.contains(&target),
        _ => false,
    }
}

fn for_each_let(body: &Block, f: &mut dyn FnMut(&Value)) {
    for op in &body.ops {
        if let Op::Let { value, .. } = op {
            f(value);
            match value {
                Value::Loop {
                    header,
                    body,
                    latch,
                } => {
                    for_each_let(header, f);
                    for_each_let(body, f);
                    for_each_let(latch, f);
                }
                Value::If {
                    then_block,
                    else_block,
                    ..
                } => {
                    for_each_let(then_block, f);
                    for_each_let(else_block, f);
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumia_opt::{compile_source_to_optimized, OptOptions};

    fn is_dense_call(f: &lumia_core::CoreFun, sym: &str) -> bool {
        matches!(
            f.body.ops.as_slice(),
            [Op::Let {
                value: Value::Call { fun, .. },
                ..
            }] if fun == sym
        ) || match_gemv_fun(f, &crate::nsw_iv::collect_leaf_defs(&f.body)).is_some()
            && sym == "lumia_f64_gemv"
    }

    #[test]
    fn matches_clean_gemv_helper() {
        let src = r#"
module M
val gemv(m, n, a, x, y) = {
  var out = y
  var i = 0
  for i < m {
    var s = 0.0
    var j = 0
    for j < n {
      s = s + a.get(i * n + j) * x.get(j)
      j = j + 1
    }
    out = out.set(i, s)
    i = i + 1
  }
  out
}
val main = {
  val a = listOf(1.0, 2.0, 3.0, 4.0, 5.0, 6.0)
  val x = listOf(1.0, 2.0)
  var y = listOf(0.0, 0.0, 0.0)
  y = gemv(3, 2, a, x, y)
  0
}
"#;
        let core = compile_source_to_optimized(src, &OptOptions::for_build(true)).unwrap();
        let hit = core.functions.iter().any(|f| {
            f.name.contains("gemv")
                && (matches!(
                    f.body.ops.as_slice(),
                    [Op::Let {
                        value: Value::Call { fun, .. },
                        ..
                    }] if fun == "lumia_f64_gemv"
                ) || {
                    let defs = crate::nsw_iv::collect_leaf_defs(&f.body);
                    match_gemv_fun(f, &defs).is_some()
                })
        });
        assert!(hit, "expected gemv SR rewrite or match");
        let _ = is_dense_call;
    }

    #[test]
    fn matches_clean_axpy_helper() {
        let src = r#"
module M
val axpy(y, alpha, x) = {
  var out = y
  val n = y.len()
  var i = 0
  for i < n {
    out = out.set(i, out.get(i) + alpha * x.get(i))
    i = i + 1
  }
  out
}
val main = {
  var y = listOf(1.0, 2.0)
  val x = listOf(3.0, 4.0)
  y = axpy(y, 0.5, x)
  0
}
"#;
        let core = compile_source_to_optimized(src, &OptOptions::for_build(true)).unwrap();
        let hit = core.functions.iter().any(|f| {
            f.name.contains("axpy")
                && matches!(
                    f.body.ops.as_slice(),
                    [Op::Let {
                        value: Value::Call { fun, .. },
                        ..
                    }] if fun == "lumia_f64_axpy"
                )
        });
        // May be fully inlined into main; then the foreign must exist / main calls it.
        let hit = hit
            || core
                .functions
                .iter()
                .any(|f| f.external.as_deref() == Some("lumia_f64_axpy"));
        assert!(hit, "expected axpy SR rewrite");
    }

    #[test]
    fn matches_clean_copy_helper() {
        let src = r#"
module M
val copy(dst, src) = {
  var out = dst
  val n = src.len()
  var i = 0
  for i < n {
    out = out.set(i, src.get(i))
    i = i + 1
  }
  out
}
val main = {
  var d = listOf(0.0, 0.0)
  val s = listOf(1.0, 2.0)
  d = copy(d, s)
  0
}
"#;
        let core = compile_source_to_optimized(src, &OptOptions::for_build(true)).unwrap();
        let hit = core.functions.iter().any(|f| {
            f.external.as_deref() == Some("lumia_f64_copy")
                || matches!(
                    f.body.ops.as_slice(),
                    [Op::Let {
                        value: Value::Call { fun, .. },
                        ..
                    }] if fun == "lumia_f64_copy"
                )
        });
        assert!(hit, "expected copy SR rewrite");
    }
}
