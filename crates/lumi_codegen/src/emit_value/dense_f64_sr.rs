//! Recognize dense `List[Float]` nests and emit `lumi_f64_*` RT kernels.
//!
//! Whole-function patterns (params = kernel args):
//! - gemv:    `y[i] = Σ_j A[i·n+j] * x[j]`
//! - gemv_t:  `y[j] += A[i·n+j] * x[i]` (after zero-fill)
//! - addmm:   `W[i,j] += α * u[i] * v[j]`
//! - axpy:    `y[i] += α * x[i]`
//! - sub/add: `out[i] = a[i] ± b[i]`
//! - mul:     `out[i] = a[i] * b[i]`
//! - scale:   `xs[i] *= α`
//! - fill:    `xs[i] = v`
//! - clamp:   `xs[i] = clamp(xs[i], lo, hi)`

use inkwell::values::FunctionValue;
use lumi_core::{
    match_add_fun, match_addmm_fun, match_axpy_fun, match_clamp_fun, match_copy_fun,
    match_fill_fun, match_gemv_fun, match_gemv_t_fun, match_mul_fun, match_scale_fun,
    match_sub_fun, DenseAddmm, DenseAxpy, DenseBin3, DenseClamp, DenseCopy, DenseFill, DenseGemv,
    DenseScale,
};

use super::super::Codegen;
use anyhow::{Context as AnyhowContext, Result};

type GemvPat = DenseGemv;
type GemvTPat = DenseGemv;
type AddmmPat = DenseAddmm;
type AxpyPat = DenseAxpy;
type SubPat = DenseBin3;
type ClampPat = DenseClamp;
type CopyPat = DenseCopy;
type ScalePat = DenseScale;
type FillPat = DenseFill;

impl<'ctx> Codegen<'ctx> {
    /// Whole-function SR for dense float helpers (params = pattern args).
    pub(crate) fn try_emit_dense_f64_fun(
        &mut self,
        fun: &lumi_core::CoreFun,
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
        if let Some(p) = match_add_fun(fun, defs) {
            return self.emit_binop3_fun("lumi_f64_add", "fadd", &p).map(Some);
        }
        if let Some(p) = match_mul_fun(fun, defs) {
            return self.emit_binop3_fun("lumi_f64_mul", "fmul", &p).map(Some);
        }
        if let Some(p) = match_clamp_fun(fun, defs) {
            return self.emit_clamp_fun(&p).map(Some);
        }
        if let Some(p) = match_scale_fun(fun, defs) {
            return self.emit_scale_fun(&p).map(Some);
        }
        if let Some(p) = match_fill_fun(fun, defs) {
            return self.emit_fill_fun(&p).map(Some);
        }
        if let Some(p) = match_copy_fun(fun, defs) {
            return self.emit_copy_fun(&p).map(Some);
        }
        Ok(None)
    }

    fn emit_gemv_fun(&mut self, p: &GemvPat) -> Result<()> {
        let rt = self.runtime_fn("lumi_f64_gemv")?;
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
        let rt = self.runtime_fn("lumi_f64_gemv_t")?;
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
        let rt = self.runtime_fn("lumi_f64_addmm")?;
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
        let rt = self.runtime_fn("lumi_f64_axpy")?;
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
        self.emit_binop3_fun("lumi_f64_sub", "fsub", p)
    }

    fn emit_binop3_fun(&mut self, sym: &str, label: &str, p: &SubPat) -> Result<()> {
        let rt = self.runtime_fn(sym)?;
        let o = self.i64_as_ptr(self.coerce_i64(self.local(p.out)?)?, "o")?;
        let a = self.i64_as_ptr(self.coerce_i64(self.local(p.a)?)?, "a")?;
        let b = self.i64_as_ptr(self.coerce_i64(self.local(p.b)?)?, "b")?;
        let call = crate::error::llvm(self.llvm.builder.build_call(
            rt,
            &[o.into(), a.into(), b.into()],
            label,
        ))?;
        let out = call
            .try_as_basic_value()
            .basic()
            .with_context(|| label.to_string())?
            .into_pointer_value();
        let out_i = self
            .ptr_as_i64(out, &format!("{label}_i64"))?
            .into_int_value();
        crate::error::llvm(self.llvm.builder.build_return(Some(&out_i)))?;
        Ok(())
    }

    fn emit_clamp_fun(&mut self, p: &ClampPat) -> Result<()> {
        let rt = self.runtime_fn("lumi_f64_clamp")?;
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

    fn emit_scale_fun(&mut self, p: &ScalePat) -> Result<()> {
        let rt = self.runtime_fn("lumi_f64_scale")?;
        let xs = self.i64_as_ptr(self.coerce_i64(self.local(p.xs)?)?, "xs")?;
        let alpha = self.promote_f64(self.local(p.alpha)?)?;
        let call = crate::error::llvm(self.llvm.builder.build_call(
            rt,
            &[xs.into(), alpha.into()],
            "fscale",
        ))?;
        let out = call
            .try_as_basic_value()
            .basic()
            .context("scale")?
            .into_pointer_value();
        let out_i = self.ptr_as_i64(out, "scale_i64")?.into_int_value();
        crate::error::llvm(self.llvm.builder.build_return(Some(&out_i)))?;
        Ok(())
    }

    fn emit_fill_fun(&mut self, p: &FillPat) -> Result<()> {
        let rt = self.runtime_fn("lumi_f64_fill")?;
        let xs = self.i64_as_ptr(self.coerce_i64(self.local(p.xs)?)?, "xs")?;
        let v = self.promote_f64(self.local(p.v)?)?;
        let call = crate::error::llvm(self.llvm.builder.build_call(
            rt,
            &[xs.into(), v.into()],
            "ffill",
        ))?;
        let out = call
            .try_as_basic_value()
            .basic()
            .context("fill")?
            .into_pointer_value();
        let out_i = self.ptr_as_i64(out, "fill_i64")?.into_int_value();
        crate::error::llvm(self.llvm.builder.build_return(Some(&out_i)))?;
        Ok(())
    }

    fn emit_copy_fun(&mut self, p: &CopyPat) -> Result<()> {
        let rt = self.runtime_fn("lumi_f64_copy")?;
        let dst = self.i64_as_ptr(self.coerce_i64(self.local(p.dst)?)?, "dst")?;
        let src = self.i64_as_ptr(self.coerce_i64(self.local(p.src)?)?, "src")?;
        let call = crate::error::llvm(self.llvm.builder.build_call(
            rt,
            &[dst.into(), src.into()],
            "fcopy",
        ))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use lumi_core::{Op, Value};
    use lumi_opt::{compile_source_to_optimized, OptOptions};

    fn is_dense_call(f: &lumi_core::CoreFun, sym: &str) -> bool {
        matches!(
            f.body.ops.as_slice(),
            [Op::Let {
                value: Value::Call { fun, .. },
                ..
            }] if fun == sym
        ) || match_gemv_fun(f, &crate::nsw_iv::collect_leaf_defs(&f.body)).is_some()
            && sym == "lumi_f64_gemv"
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
                    }] if fun == "lumi_f64_gemv"
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
                    }] if fun == "lumi_f64_axpy"
                )
        });
        // May be fully inlined into main; then the foreign must exist / main calls it.
        let hit = hit
            || core
                .functions
                .iter()
                .any(|f| f.external.as_deref() == Some("lumi_f64_axpy"));
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
            f.external.as_deref() == Some("lumi_f64_copy")
                || matches!(
                    f.body.ops.as_slice(),
                    [Op::Let {
                        value: Value::Call { fun, .. },
                        ..
                    }] if fun == "lumi_f64_copy"
                )
        });
        assert!(hit, "expected copy SR rewrite");
    }
}
