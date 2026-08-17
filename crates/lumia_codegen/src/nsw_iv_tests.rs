use super::*;
use lumia_core::compile_source_to_core;

#[test]
fn marks_lt_unit_increment() {
    let core = compile_source_to_core(
        r#"
module M
val main = {
  var i = 0
  var s = 0
  for i < 10 {
s = s + i
i = i + 1
  }
  s
}
"#,
    )
    .unwrap();
    let main = core.functions.iter().find(|f| f.name == "main").unwrap();
    let nsw = collect_nsw_binop_locals(&main.body);
    assert!(!nsw.is_empty(), "expected i=i+1 under i<10 to be NSW-safe");
}

#[test]
fn marks_shared_const_one_outside_loop() {
    // Mimic bench lowering: `1` defined once, reused inside the loop.
    let core = compile_source_to_core(
        r#"
module M
val main = {
  var i = 0
  val one = 1
  for i < 10 {
i = i + one
  }
  i
}
"#,
    )
    .unwrap();
    let main = core.functions.iter().find(|f| f.name == "main").unwrap();
    let nsw = collect_nsw_binop_locals(&main.body);
    assert!(
        !nsw.is_empty(),
        "i=i+one under i<10 should be NSW-safe even if `one` is outer"
    );
}

#[test]
fn marks_le_const_increment() {
    let core = compile_source_to_core(
        r#"
module M
val main = {
  var i = 0
  var s = 0
  for i <= 10 {
s = s + i
i = i + 1
  }
  s
}
"#,
    )
    .unwrap();
    let main = core.functions.iter().find(|f| f.name == "main").unwrap();
    let nsw = collect_nsw_binop_locals(&main.body);
    assert!(!nsw.is_empty(), "i=i+1 under i<=10 (const) is NSW-safe");
}

#[test]
fn skips_le_unknown_bound() {
    let core = compile_source_to_core(
        r#"
module M
val main(limit) = {
  var i = 0
  for i <= limit {
i = i + 1
  }
  i
}
"#,
    )
    .unwrap();
    let main = core.functions.iter().find(|f| f.name == "main").unwrap();
    let nsw = collect_nsw_binop_locals(&main.body);
    assert!(
        nsw.is_empty(),
        "i=i+1 under i<=limit (unknown) must keep overflow checks"
    );
}

#[test]
fn marks_matmul_iv_increments() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/bench_cpu.lm");
    let src = std::fs::read_to_string(&path).unwrap();
    let core =
        lumia_opt::compile_source_to_optimized(&src, &lumia_opt::OptOptions::for_build(true))
            .unwrap();
    // Prefer the const-specialized clone (`matmulChecksum$c_160`) when present.
    let f = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("matmulChecksum$c_"))
        .or_else(|| core.functions.iter().find(|f| f.name == "matmulChecksum"))
        .unwrap();
    let nsw = collect_nsw_binop_locals(&f.body);
    assert!(
        nsw.len() >= 3,
        "expected i/j/k unit steps under strict <, got {nsw:?}"
    );
}

#[test]
fn marks_const_bound_mul_tree() {
    let core = compile_source_to_core(
        r#"
module M
val main = {
  var i = 0
  var s = 0
  for i < 10 {
s = s + i * 10 + 1
i = i + 1
  }
  s
}
"#,
    )
    .unwrap();
    let main = core.functions.iter().find(|f| f.name == "main").unwrap();
    let nsw = collect_nsw_binop_locals(&main.body);
    assert!(
        nsw.len() >= 2,
        "expected unit step + i*10/+ tree under i<10, got {nsw:?}"
    );
}

#[test]
fn marks_is_prime_d_as_safe_divisor() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/bench_cpu.lm");
    let src = std::fs::read_to_string(&path).unwrap();
    let core =
        lumia_opt::compile_source_to_optimized(&src, &lumia_opt::OptOptions::for_build(true))
            .unwrap();
    let f = core
        .functions
        .iter()
        .find(|f| f.name == "countPrimes")
        .unwrap();
    let safe = collect_safe_divisor_locals(&f.body);
    assert!(
        !safe.is_empty(),
        "inlined isPrime `d` (init 2, +=1) should yield safe divisor locals"
    );
}

#[test]
fn rejects_zero_init_slot_as_divisor() {
    let core = compile_source_to_core(
        r#"
module M
val main = {
  var i = 0
  var s = 0
  for i < 10 {
s = s + (s % i)
i = i + 1
  }
  s
}
"#,
    )
    .unwrap();
    let main = core.functions.iter().find(|f| f.name == "main").unwrap();
    // `i` starts at 0 — must not treat Name(i) loads as safe divisors.
    // Int constants like the loop bound may still be marked; check no Name-based
    // safety by ensuring ge2 path does not fire for i: only Int≠{0,-1} locals.
    let safe = collect_safe_divisor_locals(&main.body);
    let ge2 = collect_ge2_unit_slots(&main.body, &lumia_core::collect_leaf_defs(&main.body, false));
    assert!(
        !ge2.contains("i"),
        "i starts at 0, got ge2={ge2:?} safe={safe:?}"
    );
}

#[test]
fn marks_collatz_x_loads_nonneg() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/bench_cpu.lm");
    let src = std::fs::read_to_string(&path).unwrap();
    let core =
        lumia_opt::compile_source_to_optimized(&src, &lumia_opt::OptOptions::for_build(true))
            .unwrap();
    let f = core
        .functions
        .iter()
        .find(|f| f.name == "collatzTotal")
        .unwrap();
    let nonneg = collect_nonneg_iv_load_locals(&f.body);
    assert!(
        !nonneg.is_empty(),
        "inlined collatzSteps `x` under x>1 should be nonneg loads"
    );
}

#[test]
fn fib_match01_subs_stay_checked() {
    let core = compile_source_to_core(
        r#"
module M
val fib(n) = {
  n match {
0 -> 0
1 -> 1
_ -> fib(n - 1) + fib(n - 2)
  }
}
val main = fib(10)
"#,
    )
    .unwrap();
    let fib = core.functions.iter().find(|f| f.name == "fib").unwrap();
    let nsw = collect_nsw_binop_locals(&fib.body);
    // Residual arm includes n=i64::MIN where n-1 overflows — keep checked.
    let defs = lumia_core::collect_leaf_defs(&fib.body, false);
    let sub_locals: Vec<_> = defs
        .iter()
        .filter_map(|(id, v)| match v {
            Value::Binary { op: BinOp::Sub, .. } => Some(*id),
            _ => None,
        })
        .collect();
    assert!(
        sub_locals.iter().all(|id| !nsw.contains(id)),
        "fib n-1/n-2 must not be NSW: {sub_locals:?} nsw={nsw:?}"
    );
    let add_locals: Vec<_> = defs
        .iter()
        .filter_map(|(id, v)| match v {
            Value::Binary { op: BinOp::Add, .. } => Some(*id),
            _ => None,
        })
        .collect();
    assert!(
        add_locals.iter().all(|id| !nsw.contains(id)),
        "fib add must not be NSW: {add_locals:?} nsw={nsw:?}"
    );
}
