//! Thin trampoline emit for dense `List[Float]` helpers rewritten by opt.
//!
//! Nest pattern matching lives only in [`lumia_opt::dense_f64_sr`]. After that
//! pass, matched helpers are a single `Call(lumia_f64_*)` (or fully inlined).
//! Codegen recognizes that Call shape and emits a frameless RT forwarder.
//!
//! Eligible symbols: [`lumia_abi::DENSE_F64_TRAMPOLINE_SYMS`].

use inkwell::values::{BasicMetadataValueEnum, FunctionValue};
use lumia_abi::{is_dense_f64_trampoline, DENSE_F64_TRAMPOLINE_SYMS};
use lumia_core::{Op, Value};
use lumia_ty::Type;

use super::super::Codegen;
use anyhow::{bail, Context as AnyhowContext, Result};

impl<'ctx> Codegen<'ctx> {
    /// Frameless emit when the body is already a dense-f64 RT `Call`.
    pub(crate) fn try_emit_dense_f64_fun(
        &mut self,
        fun: &lumia_core::CoreFun,
        _fv: FunctionValue<'ctx>,
    ) -> Result<Option<()>> {
        let Some(sym) = dense_f64_trampoline_symbol(fun) else {
            return Ok(None);
        };
        self.emit_dense_f64_trampoline(fun, sym).map(Some)
    }

    fn emit_dense_f64_trampoline(&mut self, fun: &lumia_core::CoreFun, sym: &str) -> Result<()> {
        let rt = self.runtime_fn(sym)?;
        let mut args: Vec<BasicMetadataValueEnum<'ctx>> = Vec::with_capacity(fun.params.len());
        for (i, p) in fun.params.iter().enumerate() {
            let ty = fun.param_tys.get(i).cloned().unwrap_or(Type::Int);
            let v = self.local(*p)?;
            let arg = match ty {
                Type::Float => self.promote_f64(v)?.into(),
                Type::List(_) => self
                    .i64_as_ptr(self.coerce_i64(v)?, "dense_arg")?
                    .into(),
                _ => self.coerce_i64(v)?.into(),
            };
            args.push(arg);
        }
        let call = crate::error::llvm(self.llvm.builder.build_call(rt, &args, "dense_f64"))?;
        let ret_ty = &fun.ret_ty;
        let out_i = match ret_ty {
            Type::Float => {
                let f = call
                    .try_as_basic_value()
                    .basic()
                    .with_context(|| format!("{sym} float ret"))?
                    .into_float_value();
                crate::error::llvm(self.llvm.builder.build_bit_cast(
                    f,
                    self.llvm.context.i64_type(),
                    "dense_f64_ret",
                ))?
                .into_int_value()
            }
            Type::List(_) => {
                let ptr = call
                    .try_as_basic_value()
                    .basic()
                    .with_context(|| format!("{sym} list ret"))?
                    .into_pointer_value();
                self.ptr_as_i64(ptr, "dense_f64_ret")?.into_int_value()
            }
            _ => bail!("dense_f64 trampoline: unexpected ret_ty {ret_ty:?} for {sym}"),
        };
        crate::error::llvm(self.llvm.builder.build_return(Some(&out_i)))?;
        Ok(())
    }
}

/// Body is `[Let Call { fun: sym, args == params }]` with `result` that local.
fn dense_f64_trampoline_symbol(fun: &lumia_core::CoreFun) -> Option<&'static str> {
    let [Op::Let {
        local,
        value: Value::Call { fun: callees, args },
        ..
    }] = fun.body.ops.as_slice()
    else {
        return None;
    };
    if fun.body.result != Some(*local) {
        return None;
    }
    if args.as_slice() != fun.params.as_slice() {
        return None;
    }
    let sym = DENSE_F64_TRAMPOLINE_SYMS
        .iter()
        .copied()
        .find(|s| *s == callees.as_str())?;
    debug_assert!(is_dense_f64_trampoline(sym));
    // Refuse trampoline when ret_ty is still a leftover scalar (false SR match).
    match (sym, &fun.ret_ty) {
        (
            "lumia_f64_sum_sq" | "lumia_f64_mean" | "lumia_f64_std" | "lumia_f64_l2_norm",
            Type::Float,
        ) => Some(sym),
        (
            "lumia_f64_gemv"
            | "lumia_f64_gemv_t"
            | "lumia_f64_addmm"
            | "lumia_f64_axpy"
            | "lumia_f64_sub"
            | "lumia_f64_add"
            | "lumia_f64_mul"
            | "lumia_f64_clamp"
            | "lumia_f64_scale"
            | "lumia_f64_fill"
            | "lumia_f64_copy"
            | "lumia_list_f64_zeros"
            | "lumia_f64_softmax"
            | "lumia_f64_l2_normalize",
            Type::List(_),
        ) => Some(sym),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumia_core::{Block, CoreFun, Local, FunKind};
    use lumia_opt::{compile_source_to_optimized, OptOptions};
    use lumia_ty::Effect;

    #[test]
    fn recognizes_opt_rewritten_call_body() {
        let fun = CoreFun {
            name: "gemv".into(),
            params: vec![Local(0), Local(1), Local(2), Local(3), Local(4)],
            param_names: vec!["m".into(), "n".into(), "a".into(), "x".into(), "y".into()],
            param_tys: vec![
                Type::Int,
                Type::Int,
                Type::List(Box::new(Type::Float)),
                Type::List(Box::new(Type::Float)),
                Type::List(Box::new(Type::Float)),
            ],
            ret_ty: Type::List(Box::new(Type::Float)),
            effect: Effect::pure(),
            body: Block {
                ops: vec![Op::Let {
                    local: Local(5),
                    value: Value::Call {
                        fun: "lumia_f64_gemv".into(),
                        args: vec![Local(0), Local(1), Local(2), Local(3), Local(4)],
                    },
                    pure_region: true,
                }],
                result: Some(Local(5)),
            },
            is_main: false,
            memo: None,
            external: None,
            foreign_abi: lumia_core::ForeignAbi::C,
            escaping: Default::default(),
            scheme_poly: false,
            mono_of: None,
            kind: FunKind::Normal,
        };
        assert_eq!(dense_f64_trampoline_symbol(&fun), Some("lumia_f64_gemv"));
    }

    #[test]
    fn opt_rewrites_gemv_helper() {
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
                && dense_f64_trampoline_symbol(f).is_some_and(|s| s == "lumia_f64_gemv")
                || f.external.as_deref() == Some("lumia_f64_gemv")
        });
        assert!(hit, "expected gemv SR rewrite to lumia_f64_gemv Call");
    }

    #[test]
    fn opt_rewrites_axpy_helper() {
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
            dense_f64_trampoline_symbol(f).is_some_and(|s| s == "lumia_f64_axpy")
                || f.external.as_deref() == Some("lumia_f64_axpy")
        });
        assert!(hit, "expected axpy SR rewrite");
    }

    #[test]
    fn opt_rewrites_copy_helper() {
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
                || dense_f64_trampoline_symbol(f).is_some_and(|s| s == "lumia_f64_copy")
        });
        assert!(hit, "expected copy SR rewrite");
    }
}
