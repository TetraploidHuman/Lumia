use super::*;
use lumia_core::compile_source_to_core;
use lumia_core::CoreBinOp as BinOp;

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
fn marks_nested_inclusive_named_bound() {
    // Outer `n < 100` seeds iv_upper[n]=100; inner `i <= n` may NSW `i+1` / latch +1.
    let core = compile_source_to_core(
        r#"
module M
val main = {
  var n = 0
  var s = 0
  for n < 100 {
    var i = 0
    for i <= n {
      s = s + (i + 1)
      i = i + 1
    }
    n = n + 1
  }
  s
}
"#,
    )
    .unwrap();
    let main = core.functions.iter().find(|f| f.name == "main").unwrap();
    let nsw = collect_nsw_binop_locals(&main.body);
    let defs = lumia_core::collect_leaf_defs(&main.body, false);
    let has_plus_one = defs.iter().any(|(id, v)| {
        matches!(
            v,
            Value::Binary {
                op: BinOp::Add,
                left,
                right,
                ..
            } if {
                let lc = lumia_core::const_of(*left, &defs);
                let rc = lumia_core::const_of(*right, &defs);
                (lc == Some(1) || rc == Some(1)) && nsw.contains(id)
            }
        )
    });
    assert!(
        has_plus_one,
        "expected `i + 1` under nested i<=n < 100 NSW, nsw={nsw:?}"
    );
}

#[test]
fn marks_nested_inclusive_iv_add_literal() {
    let core = compile_source_to_core(
        r#"
module M
val main = {
  var n = 0
  var s = 0
  for n < 100 {
    var i = 0
    for i <= n {
      s = s + (i + 50)
      i = i + 1
    }
    n = n + 1
  }
  s
}
"#,
    )
    .unwrap();
    let main = core.functions.iter().find(|f| f.name == "main").unwrap();
    let nsw = collect_nsw_binop_locals(&main.body);
    let defs = lumia_core::collect_leaf_defs(&main.body, false);
    let has_plus_fifty = defs.iter().any(|(id, v)| {
        matches!(
            v,
            Value::Binary {
                op: BinOp::Add,
                left,
                right,
                ..
            } if {
                let lc = lumia_core::const_of(*left, &defs);
                let rc = lumia_core::const_of(*right, &defs);
                (lc == Some(50) || rc == Some(50)) && nsw.contains(id)
            }
        )
    });
    assert!(
        has_plus_fifty,
        "expected `i + 50` under nested i<=n < 100 NSW, nsw={nsw:?}"
    );
}

#[test]
fn marks_matmul_iv_increments() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/bench/bench_cpu.lm");
    let src = std::fs::read_to_string(&path).unwrap();
    let core =
        crate::compile_source_to_optimized(&src, &crate::OptOptions::for_build(true)).unwrap();
    // Prefer the const-specialized clone (`matmulChecksum$c_160`) when present.
    let f = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("matmulChecksum$c_"))
        .or_else(|| core.functions.iter().find(|f| f.name == "matmulChecksum"))
        .unwrap();
    // domain_sr may whole-fn rewrite this helper to an RT Call — then there is
    // no IV loop left for NSW (coverage moves to `marks_triple_nest_iv_and_acc`).
    let domain_sr = f.body.ops.iter().any(|op| {
        matches!(
            op,
            lumia_core::Op::Let {
                value: lumia_core::Value::Call { fun, .. },
                ..
            } if fun == "lumia_matmul_affine_checksum"
        )
    });
    if domain_sr {
        return;
    }
    let nsw = collect_nsw_binop_locals(&f.body);
    assert!(
        nsw.len() >= 3,
        "expected i/j/k unit steps under strict <, got {nsw:?}"
    );
    // Under n≤NSW_ACC_BOUND_MAX, `cell += product` must be NSW (tree-acc bootstrap).
    // Count Binary Add locals that are not mere unit steps — product trees + cell+=.
    assert!(
        nsw.len() >= 8,
        "expected matmul product tree + cell+= NSW under ACC_BOUND, got {} locals {nsw:?}",
        nsw.len()
    );
}

#[test]
fn marks_triple_nest_iv_and_acc() {
    // Matmul-shaped IV/acc without matching domain_sr checksum (no rem / affine).
    let core = compile_source_to_core(
        r#"
module M
val main = {
  var i = 0
  var s = 0
  for i < 8 {
    var j = 0
    for j < 8 {
      var k = 0
      var cell = 0
      for k < 8 {
        cell = cell + i * j + k
        k = k + 1
      }
      s = s + cell
      j = j + 1
    }
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
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/bench/bench_cpu.lm");
    let src = std::fs::read_to_string(&path).unwrap();
    let core =
        crate::compile_source_to_optimized(&src, &crate::OptOptions::for_build(true)).unwrap();
    let f = core.functions.iter().find(|f| f.name == "isPrime").unwrap();
    let safe = collect_safe_divisor_locals(&f.body);
    assert!(
        !safe.is_empty(),
        "isPrime `d` (init 2, +=1) should yield safe divisor locals"
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
    let ge1 = collect_ge1_unit_slots(
        &main.body,
        &lumia_core::collect_leaf_defs(&main.body, false),
    );
    let ge2 = collect_ge2_unit_slots(
        &main.body,
        &lumia_core::collect_leaf_defs(&main.body, false),
    );
    assert!(
        !ge1.contains("i") && !ge2.contains("i"),
        "i starts at 0, got ge1={ge1:?} ge2={ge2:?}"
    );
}

#[test]
fn marks_ge1_unit_slot_as_safe_divisor() {
    let core = compile_source_to_core(
        r#"
module M
val main = {
  var i = 1
  var s = 0
  for i < 10 {
s = s + (10 / i)
i = i + 1
  }
  s
}
"#,
    )
    .unwrap();
    let main = core.functions.iter().find(|f| f.name == "main").unwrap();
    let defs = lumia_core::collect_leaf_defs(&main.body, false);
    let ge1 = collect_ge1_unit_slots(&main.body, &defs);
    assert!(ge1.contains("i"), "i init 1 +=1 should be ge1, got {ge1:?}");
    let safe = collect_safe_divisor_locals(&main.body);
    assert!(
        !safe.is_empty(),
        "Name(i) loads should be safe divisors when i is ge1"
    );
}

#[test]
fn marks_collatz_x_loads_nonneg() {
    // Use a helper name that domain_sr does not rewrite to RT.
    let core = compile_source_to_core(
        r#"
module M
val collatzLikeSteps(n) = {
  var x = n
  var steps = 0
  for x > 1 {
    if x % 2 == 0 {
      x = x / 2
    } else {
      x = 3 * x + 1
    }
    steps = steps + 1
  }
  steps
}
val main = collatzLikeSteps(27)
"#,
    )
    .unwrap();
    let mut core = core;
    crate::optimize(&mut core, &crate::OptOptions::for_build(true));
    let f = core
        .functions
        .iter()
        .find(|f| f.name == "collatzLikeSteps")
        .unwrap();
    let nonneg = collect_nonneg_iv_load_locals(&f.body);
    assert!(
        !nonneg.is_empty(),
        "collatzLikeSteps `x` under x>1 should be nonneg loads"
    );
}

#[test]
fn marks_nonneg_iv_sub() {
    // `i > 0` proves loads of `i` are nonneg; `i - 1` is NSW (unlike fib open `n`).
    let core = compile_source_to_core(
        r#"
module M
val main = {
  var i = 1
  var s = 0
  for i > 0 {
    s = s + (i - 1)
    i = i - 1
  }
  s
}
"#,
    )
    .unwrap();
    let main = core.functions.iter().find(|f| f.name == "main").unwrap();
    let nsw = collect_nsw_binop_locals(&main.body);
    let defs = lumia_core::collect_leaf_defs(&main.body, false);
    let has_nsw_sub = defs
        .iter()
        .any(|(id, v)| matches!(v, Value::Binary { op: BinOp::Sub, .. }) && nsw.contains(id));
    assert!(
        has_nsw_sub,
        "expected nonneg IV `i - 1` under i>0 to be NSW-safe, nsw={nsw:?}"
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

#[test]
fn marks_nonneg_const_add_mul() {
    let core = compile_source_to_core(
        r#"
module M
val main = {
  val a = 3 + 4
  val b = 5 * 6
  a + b
}
"#,
    )
    .unwrap();
    let main = core.functions.iter().find(|f| f.name == "main").unwrap();
    let nsw = collect_nsw_binop_locals(&main.body);
    let defs = lumia_core::collect_leaf_defs(&main.body, false);
    let add_ok = defs
        .iter()
        .any(|(id, v)| matches!(v, Value::Binary { op: BinOp::Add, .. }) && nsw.contains(id));
    let mul_ok = defs
        .iter()
        .any(|(id, v)| matches!(v, Value::Binary { op: BinOp::Mul, .. }) && nsw.contains(id));
    assert!(add_ok, "3+4 should be NSW: nsw={nsw:?}");
    assert!(mul_ok, "5*6 should be NSW: nsw={nsw:?}");
}

#[test]
fn marks_bounded_iv_add_mul_literal() {
    // `i < 100` seeds iv_upper; `i + 50` / `i * 3` must be NSW (U.checked_* ok).
    let core = compile_source_to_core(
        r#"
module M
val main = {
  var i = 0
  var s = 0
  for i < 100 {
    s = s + (i + 50)
    s = s + (i * 3)
    i = i + 1
  }
  s
}
"#,
    )
    .unwrap();
    let main = core.functions.iter().find(|f| f.name == "main").unwrap();
    let nsw = collect_nsw_binop_locals(&main.body);
    let defs = lumia_core::collect_leaf_defs(&main.body, false);
    let has_iv_plus = defs.iter().any(|(id, v)| {
        matches!(
            v,
            Value::Binary {
                op: BinOp::Add,
                left,
                right,
                ..
            } if {
                let lc = lumia_core::const_of(*left, &defs);
                let rc = lumia_core::const_of(*right, &defs);
                (lc == Some(50) || rc == Some(50)) && nsw.contains(id)
            }
        )
    });
    let has_iv_mul = defs.iter().any(|(id, v)| {
        matches!(
            v,
            Value::Binary {
                op: BinOp::Mul,
                left,
                right,
                ..
            } if {
                let lc = lumia_core::const_of(*left, &defs);
                let rc = lumia_core::const_of(*right, &defs);
                (lc == Some(3) || rc == Some(3)) && nsw.contains(id)
            }
        )
    });
    assert!(
        has_iv_plus,
        "expected `i + 50` under i<100 NSW, nsw={nsw:?}"
    );
    assert!(has_iv_mul, "expected `i * 3` under i<100 NSW, nsw={nsw:?}");
}

#[test]
fn skips_open_lower_bound_iv_add_literal() {
    // `i > 0` proves nonneg but no const upper — `i + 50` must stay checked.
    let core = compile_source_to_core(
        r#"
module M
val main = {
  var i = 10
  var s = 0
  for i > 0 {
    s = s + (i + 50)
    i = i - 1
  }
  s
}
"#,
    )
    .unwrap();
    let main = core.functions.iter().find(|f| f.name == "main").unwrap();
    let nsw = collect_nsw_binop_locals(&main.body);
    let defs = lumia_core::collect_leaf_defs(&main.body, false);
    let bad = defs.iter().any(|(id, v)| {
        matches!(
            v,
            Value::Binary {
                op: BinOp::Add,
                left,
                right,
                ..
            } if {
                let lc = lumia_core::const_of(*left, &defs);
                let rc = lumia_core::const_of(*right, &defs);
                (lc == Some(50) || rc == Some(50)) && nsw.contains(id)
            }
        )
    });
    assert!(!bad, "`i + 50` under open i>0 must not be NSW, nsw={nsw:?}");
}

#[test]
fn marks_open_exclusive_iv_plus_one() {
    // Open `i < limit`: worst-case max i is MAX-1 ⇒ `i + 1` NSW; `i + 50` not.
    let core = compile_source_to_core(
        r#"
module M
val main(limit) = {
  var i = 0
  var s = 0
  for i < limit {
    s = s + (i + 1)
    s = s + (i + 50)
    i = i + 1
  }
  s
}
"#,
    )
    .unwrap();
    let main = core.functions.iter().find(|f| f.name == "main").unwrap();
    let nsw = collect_nsw_binop_locals(&main.body);
    let defs = lumia_core::collect_leaf_defs(&main.body, false);
    let has_plus_one = defs.iter().any(|(id, v)| {
        matches!(
            v,
            Value::Binary {
                op: BinOp::Add,
                left,
                right,
                ..
            } if {
                let lc = lumia_core::const_of(*left, &defs);
                let rc = lumia_core::const_of(*right, &defs);
                (lc == Some(1) || rc == Some(1)) && nsw.contains(id)
            }
        )
    });
    let has_plus_fifty = defs.iter().any(|(id, v)| {
        matches!(
            v,
            Value::Binary {
                op: BinOp::Add,
                left,
                right,
                ..
            } if {
                let lc = lumia_core::const_of(*left, &defs);
                let rc = lumia_core::const_of(*right, &defs);
                (lc == Some(50) || rc == Some(50)) && nsw.contains(id)
            }
        )
    });
    assert!(
        has_plus_one,
        "expected `i + 1` under open i<limit NSW, nsw={nsw:?}"
    );
    assert!(
        !has_plus_fifty,
        "`i + 50` under open i<limit must stay checked, nsw={nsw:?}"
    );
}

#[test]
fn marks_open_loop_safe_div() {
    let core = compile_source_to_core(
        r#"
module M
val main(n) = {
  var i = 1
  var s = 0
  for i < n {
    s = s + (n / i)
    i = i + 1
  }
  s
}
"#,
    )
    .unwrap();
    let main = core.functions.iter().find(|f| f.name == "main").unwrap();
    let nsw = collect_nsw_binop_locals(&main.body);
    let defs = lumia_core::collect_leaf_defs(&main.body, false);
    let has_div = defs
        .iter()
        .any(|(id, v)| matches!(v, Value::Binary { op: BinOp::Div, .. }) && nsw.contains(id));
    assert!(
        has_div,
        "expected safe `n / i` NSW under open loop, nsw={nsw:?}"
    );
}

#[test]
fn skips_i64_max_plus_one_const_add() {
    let core = compile_source_to_core(
        r#"
module M
val main = {
  val hi = 9223372036854775807
  hi + 1
}
"#,
    )
    .unwrap();
    let main = core.functions.iter().find(|f| f.name == "main").unwrap();
    let nsw = collect_nsw_binop_locals(&main.body);
    let defs = lumia_core::collect_leaf_defs(&main.body, false);
    let overflowing_add = defs
        .iter()
        .any(|(id, v)| matches!(v, Value::Binary { op: BinOp::Add, .. }) && nsw.contains(id));
    assert!(
        !overflowing_add,
        "i64::MAX + 1 must not be NSW: nsw={nsw:?}"
    );
}

#[test]
fn marks_bounded_nonneg_pair_add() {
    let core = compile_source_to_core(
        r#"
module M
val main = {
  var i = 0
  var j = 0
  var s = 0
  for i < 10 {
    for j < 10 {
      val t = i + j
      s = s + t
      j = j + 1
    }
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
        !nsw.is_empty(),
        "i+j under bounded nonneg IVs should be NSW-safe"
    );
    let defs = lumia_core::collect_leaf_defs(&main.body, false);
    let pair = defs.iter().any(|(id, v)| {
        let Value::Binary {
            op: BinOp::Add,
            left,
            right,
            ..
        } = v
        else {
            return false;
        };
        if !nsw.contains(id) {
            return false;
        }
        let ln = lumia_core::name_of(*left, &defs);
        let rn = lumia_core::name_of(*right, &defs);
        matches!(
            (ln.as_deref(), rn.as_deref()),
            (Some("i"), Some("j")) | (Some("j"), Some("i"))
        )
    });
    assert!(pair, "expected the `i + j` binop itself to be NSW-marked");
}

#[test]
fn marks_bounded_nonneg_pair_mul() {
    let core = compile_source_to_core(
        r#"
module M
val main = {
  var i = 0
  var j = 0
  var s = 0
  for i < 10 {
    for j < 10 {
      val t = i * j
      s = s + t
      j = j + 1
    }
    i = i + 1
  }
  s
}
"#,
    )
    .unwrap();
    let main = core.functions.iter().find(|f| f.name == "main").unwrap();
    let nsw = collect_nsw_binop_locals(&main.body);
    let defs = lumia_core::collect_leaf_defs(&main.body, false);
    let pair = defs.iter().any(|(id, v)| {
        let Value::Binary {
            op: BinOp::Mul,
            left,
            right,
            ..
        } = v
        else {
            return false;
        };
        if !nsw.contains(id) {
            return false;
        }
        let ln = lumia_core::name_of(*left, &defs);
        let rn = lumia_core::name_of(*right, &defs);
        matches!(
            (ln.as_deref(), rn.as_deref()),
            (Some("i"), Some("j")) | (Some("j"), Some("i"))
        )
    });
    assert!(pair, "expected the `i * j` binop itself to be NSW-marked");
}
