//! Rewrite dense `List[Float]` helpers to `lumi_f64_*` foreign calls (before Inline).
//!
//! Whole-function patterns become a single `Call` so Release inlining places the
//! RT kernel at the call site (same shape as `std.linalg` wrappers).
//!
//! Covered: gemv/gemvT/addmm/axpy/sub/add/mul/clamp/scale/fill/copy/zeros,
//! plus sumSq/mean/std/l2Norm/l2Normalize/softMax (scalar `sqrtF`/`expF` foreign
//! calls unlock the latter norms).

use lumi_core::{
    collect_leaf_defs, match_add_fun, match_addmm_fun, match_axpy_fun, match_clamp_fun,
    match_copy_fun, match_fill_fun, match_gemv_fun, match_gemv_t_fun, match_l2_norm_fun,
    match_l2_normalize_fun, match_mean_fun, match_mul_fun, match_scale_fun, match_softmax_fun,
    match_std_fun, match_sub_fun, match_sum_sq_fun, match_zeros_fun, max_local_in_fun, Block,
    CoreFun, CoreModule, Local, Op, Value,
};
use lumi_ty::{Effect, Type};
use rustc_hash::FxHashSet as HashSet;

pub struct DenseF64SrPass;

impl crate::Pass for DenseF64SrPass {
    fn name(&self) -> &str {
        "dense_f64_sr"
    }
    fn run(&self, module: &mut CoreModule) {
        dense_f64_sr_module(module);
    }
}

fn dense_f64_sr_module(module: &mut CoreModule) {
    let mut rewrites: Vec<(usize, &'static str)> = Vec::new();
    for (i, fun) in module.functions.iter().enumerate() {
        if fun.external.is_some() || fun.is_main || fun.memo.is_some() {
            continue;
        }
        let defs = collect_leaf_defs(&fun.body);
        let sym = if match_gemv_fun(fun, &defs).is_some() {
            Some("lumi_f64_gemv")
        } else if match_gemv_t_fun(fun, &defs).is_some() {
            Some("lumi_f64_gemv_t")
        } else if match_addmm_fun(fun, &defs).is_some() {
            Some("lumi_f64_addmm")
        } else if match_axpy_fun(fun, &defs).is_some() {
            Some("lumi_f64_axpy")
        } else if match_sub_fun(fun, &defs).is_some() {
            Some("lumi_f64_sub")
        } else if match_add_fun(fun, &defs).is_some() {
            Some("lumi_f64_add")
        } else if match_mul_fun(fun, &defs).is_some() {
            Some("lumi_f64_mul")
        } else if match_clamp_fun(fun, &defs).is_some() {
            Some("lumi_f64_clamp")
        } else if match_scale_fun(fun, &defs).is_some() {
            Some("lumi_f64_scale")
        } else if match_fill_fun(fun, &defs).is_some() {
            Some("lumi_f64_fill")
        } else if match_copy_fun(fun, &defs).is_some() {
            Some("lumi_f64_copy")
        } else if match_zeros_fun(fun, &defs).is_some() {
            Some("lumi_list_f64_zeros")
        } else if match_l2_normalize_fun(fun, &defs).is_some() {
            Some("lumi_f64_l2_normalize")
        } else if match_softmax_fun(fun, &defs).is_some() {
            Some("lumi_f64_softmax")
        } else if match_l2_norm_fun(fun, &defs).is_some() {
            Some("lumi_f64_l2_norm")
        } else if match_std_fun(fun, &defs).is_some() {
            Some("lumi_f64_std")
        } else if match_sum_sq_fun(fun, &defs).is_some() {
            Some("lumi_f64_sum_sq")
        } else if match_mean_fun(fun, &defs).is_some() {
            Some("lumi_f64_mean")
        } else {
            None
        };
        if let Some(s) = sym {
            rewrites.push((i, s));
        }
    }
    if rewrites.is_empty() {
        return;
    }
    let mut need: HashSet<&'static str> = HashSet::default();
    for &(_, s) in &rewrites {
        need.insert(s);
    }
    for sym in need {
        ensure_external(module, sym);
    }
    for (i, sym) in rewrites {
        rewrite_body_to_call(&mut module.functions[i], sym);
    }
}

fn ensure_external(module: &mut CoreModule, sym: &str) {
    if module
        .functions
        .iter()
        .any(|f| f.name == sym || f.external.as_deref() == Some(sym))
    {
        return;
    }
    let (param_tys, ret_ty) = external_sig(sym);
    let n = param_tys.len();
    let params: Vec<Local> = (0..n as u32).map(Local).collect();
    let param_names: Vec<String> = (0..n).map(|i| format!("a{i}")).collect();
    module.functions.push(CoreFun {
        name: sym.to_string(),
        params,
        param_names,
        param_tys,
        body: Block {
            params: vec![],
            ops: vec![],
            result: None,
        },
        ret_ty,
        effect: Effect::pure(),
        is_main: false,
        memo: None,
        external: Some(sym.to_string()),
        escaping: HashSet::default(),
        scheme_poly: false,
        mono_of: None,
    });
}

fn external_sig(sym: &str) -> (Vec<Type>, Type) {
    let lf = Type::List(Box::new(Type::Float));
    match sym {
        "lumi_f64_gemv" | "lumi_f64_gemv_t" => (
            vec![Type::Int, Type::Int, lf.clone(), lf.clone(), lf.clone()],
            lf,
        ),
        "lumi_f64_addmm" => (
            vec![
                Type::Int,
                Type::Int,
                lf.clone(),
                lf.clone(),
                lf.clone(),
                Type::Float,
            ],
            lf,
        ),
        "lumi_f64_axpy" => (vec![lf.clone(), Type::Float, lf.clone()], lf),
        "lumi_f64_sub" | "lumi_f64_add" | "lumi_f64_mul" => {
            (vec![lf.clone(), lf.clone(), lf.clone()], lf)
        }
        "lumi_f64_clamp" => (vec![lf.clone(), Type::Float, Type::Float], lf),
        "lumi_f64_scale" | "lumi_f64_fill" => (vec![lf.clone(), Type::Float], lf),
        "lumi_f64_copy" => (vec![lf.clone(), lf.clone()], lf),
        "lumi_list_f64_zeros" => (vec![Type::Int], lf),
        "lumi_f64_sum_sq" | "lumi_f64_mean" | "lumi_f64_std" | "lumi_f64_l2_norm" => {
            (vec![lf], Type::Float)
        }
        "lumi_f64_softmax" => (vec![lf.clone()], lf),
        "lumi_f64_l2_normalize" => (vec![lf.clone(), Type::Float], lf),
        "lumi_f64_sqrt" | "lumi_f64_exp" => (vec![Type::Float], Type::Float),
        _ => (vec![], Type::Int),
    }
}

fn rewrite_body_to_call(fun: &mut CoreFun, sym: &str) {
    let r = Local(max_local_in_fun(fun).saturating_add(1));
    fun.body = Block {
        params: vec![],
        ops: vec![Op::Let {
            local: r,
            value: Value::Call {
                fun: sym.to_string(),
                args: fun.params.clone(),
            },
            pure_region: true,
        }],
        result: Some(r),
    };
    // Keep typed as list/float so codegen roots / ABI stay correct.
    fun.effect = Effect::pure();
}

#[cfg(test)]
mod tests {
    use crate::{optimize, OptOptions};

    #[test]
    fn rewrites_gemv_helper_to_foreign_call() {
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
        let mut core = lumi_core::compile_source_to_core(src).unwrap();
        optimize(&mut core, &OptOptions::for_build(true));
        assert!(
            core.functions
                .iter()
                .any(|f| f.external.as_deref() == Some("lumi_f64_gemv")),
            "expected injected lumi_f64_gemv foreign"
        );
    }

    #[test]
    fn rewrites_zeros_helper_to_foreign_call() {
        let src = r#"
module M
val nZeros(n) = {
  var xs = listOf(0.0)
  var i = 1
  for i < n {
    xs = xs.append(0.0)
    i = i + 1
  }
  xs
}
val main = {
  val z = nZeros(4)
  z.len()
}
"#;
        let mut core = lumi_core::compile_source_to_core(src).unwrap();
        optimize(&mut core, &OptOptions::for_build(true));
        assert!(
            core.functions
                .iter()
                .any(|f| f.external.as_deref() == Some("lumi_list_f64_zeros")),
            "expected injected lumi_list_f64_zeros foreign"
        );
    }

    #[test]
    fn rewrites_sum_sq_and_mean_helpers() {
        let src = r#"
module M
val nSumSq(xs) = {
  var s = 0.0
  var i = 0
  val n = xs.len()
  for i < n {
    val v = xs.get(i)
    s = s + v * v
    i = i + 1
  }
  s
}
val nMean(xs) = {
  var s = 0.0
  var i = 0
  val n = xs.len()
  for i < n {
    s = s + xs.get(i)
    i = i + 1
  }
  if n == 0 { 0.0 } else { s / (0.0 + n) }
}
val main = {
  val xs = listOf(1.0, 2.0, 3.0)
  val a = nSumSq(xs)
  val b = nMean(xs)
  0
}
"#;
        let mut core = lumi_core::compile_source_to_core(src).unwrap();
        optimize(&mut core, &OptOptions::for_build(true));
        let ext: Vec<_> = core
            .functions
            .iter()
            .filter_map(|f| f.external.as_deref())
            .collect();
        assert!(
            ext.contains(&"lumi_f64_sum_sq"),
            "sum_sq missing in {ext:?}"
        );
        assert!(ext.contains(&"lumi_f64_mean"), "mean missing in {ext:?}");
    }

    #[test]
    fn rewrites_l2_norm_with_sqrt_foreign() {
        let src = r#"
module M
foreign "C" pure fn lumi_f64_sqrt(x: Float) -> Float
val nL2(xs) = {
  var s = 0.0
  var i = 0
  val n = xs.len()
  for i < n {
    val v = xs.get(i)
    s = s + v * v
    i = i + 1
  }
  lumi_f64_sqrt(s)
}
val main = {
  val xs = listOf(3.0, 4.0)
  nL2(xs)
}
"#;
        let mut core = lumi_core::compile_source_to_core(src).unwrap();
        optimize(&mut core, &OptOptions::for_build(true));
        assert!(
            core.functions
                .iter()
                .any(|f| f.external.as_deref() == Some("lumi_f64_l2_norm")),
            "expected lumi_f64_l2_norm"
        );
    }

    #[test]
    fn rewrites_l2_normalize_and_softmax() {
        let src = r#"
module M
foreign "C" pure fn lumi_f64_sqrt(x: Float) -> Float
foreign "C" pure fn lumi_f64_exp(x: Float) -> Float
val nNorm(xs, eps) = {
  var out = xs
  var s = 0.0
  var i = 0
  val n = out.len()
  for i < n {
    val v = out.get(i)
    s = s + v * v
    i = i + 1
  }
  val inv = 1.0 / (lumi_f64_sqrt(s) + eps)
  i = 0
  for i < n {
    out = out.set(i, out.get(i) * inv)
    i = i + 1
  }
  out
}
val nSoft(xs) = {
  var out = xs
  var m = out.get(0)
  var i = 1
  val n = out.len()
  for i < n {
    val v = out.get(i)
    if v > m { m = v }
    i = i + 1
  }
  var z = 0.0
  i = 0
  for i < n {
    val e = lumi_f64_exp(out.get(i) - m)
    out = out.set(i, e)
    z = z + e
    i = i + 1
  }
  i = 0
  for i < n {
    out = out.set(i, out.get(i) / z)
    i = i + 1
  }
  out
}
val main = {
  var xs = listOf(1.0, 2.0, 3.0)
  xs = nNorm(xs, 0.001)
  xs = nSoft(xs)
  0
}
"#;
        let mut core = lumi_core::compile_source_to_core(src).unwrap();
        optimize(&mut core, &OptOptions::for_build(true));
        let ext: Vec<_> = core
            .functions
            .iter()
            .filter_map(|f| f.external.as_deref())
            .collect();
        assert!(
            ext.contains(&"lumi_f64_l2_normalize"),
            "normalize missing in {ext:?}"
        );
        assert!(
            ext.contains(&"lumi_f64_softmax"),
            "softmax missing in {ext:?}"
        );
    }
}
