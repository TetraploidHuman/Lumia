//! Rewrite dense `List[Float]` helpers to `lumi_f64_*` foreign calls (before Inline).
//!
//! Whole-function patterns become a single `Call` so Release inlining places the
//! RT kernel at the call site (same shape as `std.linalg` wrappers).
//!
//! Covered: gemv/gemvT/addmm/axpy/sub/add/mul/clamp/scale/fill/copy/zeros,
//! plus sumSq/mean/std/l2Norm/l2Normalize/softMax (scalar `sqrtF`/`expF` foreign
//! calls unlock the latter norms).

use lumi_core::{
    collect_leaf_defs, first_assign_from_local, for_each_def_and_let, for_each_let,
    header_lt_bound, is_list_get, is_list_set, is_nontrivial_add_or_sub, list_arg_is,
    match_add_fun, match_addmm_fun, match_axpy_fun, match_clamp_fun, match_copy_fun,
    match_fill_fun, match_gemv_fun, match_gemv_t_fun, match_mul_fun, match_scale_fun,
    match_sub_fun, max_local_in_fun, mentions_local, name_of, same_local, Block, CoreFun,
    CoreModule, Local, Op, Value,
};
use lumi_hir::Builtin;
use lumi_syntax::BinOp;
use lumi_ty::{Effect, Type};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

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

/// `∑ xᵢ²` — get + self-mul + add, no set/div/sqrt.
fn match_sum_sq_fun(fun: &lumi_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 1 || !matches!(fun.ret_ty, Type::Float) {
        return None;
    }
    let xs = fun.params[0];
    if !fun_has_sum_sq_shape(&fun.body, defs, xs) {
        return None;
    }
    if body_calls_any(&fun.body, &["lumi_f64_sqrt", "sqrtF", "sqrt"]) {
        return None;
    }
    Some(())
}

/// Arithmetic mean — get + add + div, no set/mul.
fn match_mean_fun(fun: &lumi_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 1 || !matches!(fun.ret_ty, Type::Float) {
        return None;
    }
    let xs = fun.params[0];
    if !fun_has_mean_shape(&fun.body, defs, xs) {
        return None;
    }
    Some(())
}

/// `√(∑ xᵢ²)` via scalar `lumi_f64_sqrt` / `sqrt`.
fn match_l2_norm_fun(fun: &lumi_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 1 || !matches!(fun.ret_ty, Type::Float) {
        return None;
    }
    let xs = fun.params[0];
    if !fun_has_sum_sq_shape(&fun.body, defs, xs) {
        return None;
    }
    if !body_calls_any(&fun.body, &["lumi_f64_sqrt", "sqrtF", "sqrt"]) {
        return None;
    }
    Some(())
}

/// Population std: variance loop + sqrt (has nontrivial sub).
fn match_std_fun(fun: &lumi_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 1 || !matches!(fun.ret_ty, Type::Float) {
        return None;
    }
    let xs = fun.params[0];
    if !fun_has_std_shape(&fun.body, defs, xs) {
        return None;
    }
    Some(())
}

/// In-place L2 normalize with `eps` (set + sqrt + mentions eps).
fn match_l2_normalize_fun(fun: &lumi_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 2 {
        return None;
    }
    let (xs, eps) = (fun.params[0], fun.params[1]);
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, xs)?;
    if !fun_has_l2_normalize_shape(body, defs, &out_slot, eps) {
        return None;
    }
    Some(())
}

/// Softmax: max pass + exp + normalize (set + exp call + Gt).
fn match_softmax_fun(fun: &lumi_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 1 {
        return None;
    }
    let xs = fun.params[0];
    let body = &fun.body;
    let out_slot = first_assign_from_local(body, xs)?;
    if !fun_has_softmax_shape(body, defs, &out_slot) {
        return None;
    }
    Some(())
}

/// `zeros(n)` via `listOf(0.0)` + `append(0.0)` loop (or empty + append from 0).
fn match_zeros_fun(fun: &lumi_core::CoreFun, defs: &HashMap<u32, Value>) -> Option<()> {
    if fun.params.len() != 1 {
        return None;
    }
    let n = fun.params[0];
    let body = &fun.body;
    // Must allocate a float list seed and append 0.0 in a loop bounded by n.
    let mut seed = false;
    let mut append0 = false;
    let mut bound_n = false;
    for_each_def_and_let(body, defs, &mut |v| {
        if let Value::AllocList { elems, .. } = v {
            if elems.len() <= 1
                && elems
                    .iter()
                    .all(|e| matches!(defs.get(&e.0), Some(Value::Float(f)) if *f == 0.0))
            {
                seed = true;
            }
        }
        if let Value::Builtin {
            name: Builtin::ListAppend,
            args,
        } = v
        {
            if args.len() == 2 && matches!(defs.get(&args[1].0), Some(Value::Float(f)) if *f == 0.0)
            {
                append0 = true;
            }
        }
        if let Value::Binary {
            op: BinOp::Lt,
            right,
            ..
        } = v
        {
            if same_local(*right, n, defs) {
                bound_n = true;
            }
        }
        if let Value::Loop { header, .. } = v {
            if let Some((_, bound)) = header_lt_bound(header, defs) {
                if same_local(bound, n, defs) {
                    bound_n = true;
                }
            }
        }
    });
    if seed && append0 && bound_n {
        Some(())
    } else {
        None
    }
}

fn fun_has_sum_sq_shape(body: &Block, defs: &HashMap<u32, Value>, xs: Local) -> bool {
    let mut get = false;
    let mut mul = false;
    let mut add = false;
    let mut set = false;
    let mut div = false;
    for_each_def_and_let(body, defs, &mut |v| {
        if let Some((lst, _)) = is_list_get(v) {
            if list_arg_is(lst, xs, defs) {
                get = true;
            }
        }
        if matches!(v, Value::Binary { op: BinOp::Mul, .. }) {
            mul = true;
        }
        if is_nontrivial_add_or_sub(v, defs) && matches!(v, Value::Binary { op: BinOp::Add, .. }) {
            add = true;
        }
        if matches!(v, Value::Binary { op: BinOp::Div, .. }) {
            div = true;
        }
        if is_list_set(v).is_some() {
            set = true;
        }
    });
    get && mul && add && !set && !div
}

fn fun_has_mean_shape(body: &Block, defs: &HashMap<u32, Value>, xs: Local) -> bool {
    let mut get = false;
    let mut add = false;
    let mut div = false;
    let mut mul = false;
    let mut set = false;
    for_each_def_and_let(body, defs, &mut |v| {
        if let Some((lst, _)) = is_list_get(v) {
            if list_arg_is(lst, xs, defs) {
                get = true;
            }
        }
        if is_nontrivial_add_or_sub(v, defs) && matches!(v, Value::Binary { op: BinOp::Add, .. }) {
            add = true;
        }
        if matches!(v, Value::Binary { op: BinOp::Div, .. }) {
            div = true;
        }
        if matches!(v, Value::Binary { op: BinOp::Mul, .. }) {
            mul = true;
        }
        if is_list_set(v).is_some() {
            set = true;
        }
    });
    get && add && div && !mul && !set
}

fn fun_has_std_shape(body: &Block, defs: &HashMap<u32, Value>, xs: Local) -> bool {
    let mut get = false;
    let mut sub = false;
    let mut mul = false;
    let mut div = false;
    let mut set = false;
    for_each_def_and_let(body, defs, &mut |v| {
        if let Some((lst, _)) = is_list_get(v) {
            if list_arg_is(lst, xs, defs) {
                get = true;
            }
        }
        if matches!(v, Value::Binary { op: BinOp::Sub, .. }) {
            sub = true;
        }
        if matches!(v, Value::Binary { op: BinOp::Mul, .. }) {
            mul = true;
        }
        if matches!(v, Value::Binary { op: BinOp::Div, .. }) {
            div = true;
        }
        if is_list_set(v).is_some() {
            set = true;
        }
    });
    get && sub && mul && div && !set && body_calls_any(body, &["lumi_f64_sqrt", "sqrtF", "sqrt"])
}

fn fun_has_l2_normalize_shape(
    body: &Block,
    defs: &HashMap<u32, Value>,
    out_slot: &str,
    eps: Local,
) -> bool {
    let mut get = false;
    let mut set = false;
    let mut mul = false;
    let mut uses_eps = false;
    for_each_def_and_let(body, defs, &mut |v| {
        if let Some((lst, _)) = is_list_get(v) {
            if name_of(lst, defs).as_deref() == Some(out_slot) {
                get = true;
            }
        }
        if is_list_set(v).is_some() {
            set = true;
        }
        if matches!(v, Value::Binary { op: BinOp::Mul, .. }) {
            mul = true;
        }
        if mentions_local(v, eps) {
            uses_eps = true;
        }
    });
    get && set && mul && uses_eps && body_calls_any(body, &["lumi_f64_sqrt", "sqrtF", "sqrt"])
}

fn fun_has_softmax_shape(body: &Block, defs: &HashMap<u32, Value>, out_slot: &str) -> bool {
    let mut get = false;
    let mut set = false;
    let mut div = false;
    let mut gt = false;
    for_each_def_and_let(body, defs, &mut |v| {
        if let Some((lst, _)) = is_list_get(v) {
            if name_of(lst, defs).as_deref() == Some(out_slot) {
                get = true;
            }
        }
        if is_list_set(v).is_some() {
            set = true;
        }
        if matches!(v, Value::Binary { op: BinOp::Div, .. }) {
            div = true;
        }
        if matches!(v, Value::Binary { op: BinOp::Gt, .. }) {
            gt = true;
        }
        // max-pass update often uses If
        if matches!(v, Value::If { .. }) {
            gt = true;
        }
    });
    get && set && div && gt && body_calls_any(body, &["lumi_f64_exp", "expF", "exp"])
}

fn body_calls_any(body: &Block, names: &[&str]) -> bool {
    let mut found = false;
    for_each_let(body, &mut |val| {
        if let Value::Call { fun, .. } = val {
            if names.iter().any(|n| fun == n) {
                found = true;
            }
        }
    });
    found
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
