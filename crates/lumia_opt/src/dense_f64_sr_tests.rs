use super::external_sig;
use crate::{optimize, OptOptions};
use lumia_abi::DENSE_F64_TRAMPOLINE_SYMS;
use lumia_ty::Type;

#[test]
fn trampoline_syms_have_external_sigs() {
    for sym in DENSE_F64_TRAMPOLINE_SYMS {
        let (params, ret) = external_sig(sym);
        assert!(
            !params.is_empty() || matches!(ret, Type::List(_)),
            "{sym}: unexpected empty Int fallback"
        );
        assert!(
            !matches!(ret, Type::Int),
            "{sym}: ret should not be soft Int"
        );
    }
}

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
    let mut core = lumia_core::compile_source_to_core(src).unwrap();
    optimize(&mut core, &OptOptions::for_build(true));
    assert!(
        core.functions
            .iter()
            .any(|f| f.external.as_deref() == Some("lumia_f64_gemv")),
        "expected injected lumia_f64_gemv foreign"
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
    let mut core = lumia_core::compile_source_to_core(src).unwrap();
    optimize(&mut core, &OptOptions::for_build(true));
    assert!(
        core.functions
            .iter()
            .any(|f| f.external.as_deref() == Some("lumia_list_f64_zeros")),
        "expected injected lumia_list_f64_zeros foreign"
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
    let mut core = lumia_core::compile_source_to_core(src).unwrap();
    optimize(&mut core, &OptOptions::for_build(true));
    let ext: Vec<_> = core
        .functions
        .iter()
        .filter_map(|f| f.external.as_deref())
        .collect();
    assert!(
        ext.contains(&"lumia_f64_sum_sq"),
        "sum_sq missing in {ext:?}"
    );
    assert!(ext.contains(&"lumia_f64_mean"), "mean missing in {ext:?}");
}

#[test]
fn rewrites_l2_norm_with_sqrt_foreign() {
    let src = r#"
module M
foreign "C" pure fn lumia_f64_sqrt(x: Float) -> Float
val nL2(xs) = {
  var s = 0.0
  var i = 0
  val n = xs.len()
  for i < n {
val v = xs.get(i)
s = s + v * v
i = i + 1
  }
  lumia_f64_sqrt(s)
}
val main = {
  val xs = listOf(3.0, 4.0)
  nL2(xs)
}
"#;
    let mut core = lumia_core::compile_source_to_core(src).unwrap();
    optimize(&mut core, &OptOptions::for_build(true));
    assert!(
        core.functions
            .iter()
            .any(|f| f.external.as_deref() == Some("lumia_f64_l2_norm")),
        "expected lumia_f64_l2_norm"
    );
}

#[test]
fn rewrites_l2_normalize_and_softmax() {
    let src = r#"
module M
foreign "C" pure fn lumia_f64_sqrt(x: Float) -> Float
foreign "C" pure fn lumia_f64_exp(x: Float) -> Float
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
  val inv = 1.0 / (lumia_f64_sqrt(s) + eps)
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
val e = lumia_f64_exp(out.get(i) - m)
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
    let mut core = lumia_core::compile_source_to_core(src).unwrap();
    optimize(&mut core, &OptOptions::for_build(true));
    let ext: Vec<_> = core
        .functions
        .iter()
        .filter_map(|f| f.external.as_deref())
        .collect();
    assert!(
        ext.contains(&"lumia_f64_l2_normalize"),
        "normalize missing in {ext:?}"
    );
    assert!(
        ext.contains(&"lumia_f64_softmax"),
        "softmax missing in {ext:?}"
    );
}

#[test]
fn int_strided_loop_is_not_rewritten_to_clamp() {
    // Regression: bare arity/shape once matched Int loops (bench_cpu
    // `collatzStrided`) and rewrote them to `lumia_f64_clamp`.
    let src = r#"
module M
val steps(n) = {
  var x = n
  var c = 0
  for x > 1 {
if x % 2 == 0 {
  x = x / 2
} else {
  x = 3 * x + 1
}
c = c + 1
  }
  c
}
val strided(start, limit, stride) = {
  var n = start
  var total = 0
  for n <= limit {
total = total + steps(n)
n = n + stride
  }
  total
}
val main = {
  strided(1, 20, 3)
}
"#;
    let mut core = lumia_core::compile_source_to_core(src).unwrap();
    optimize(&mut core, &OptOptions::for_build(true));
    assert!(
        core.functions
            .iter()
            .all(|f| f.external.as_deref() != Some("lumia_f64_clamp")),
        "Int strided loop must not inject lumia_f64_clamp"
    );
    let strided = core
        .functions
        .iter()
        .find(|f| f.name == "strided")
        .expect("strided");
    let body = format!("{:?}", strided.body);
    assert!(
        !body.contains("lumia_f64_clamp"),
        "strided body must not call lumia_f64_clamp: {body}"
    );
}
