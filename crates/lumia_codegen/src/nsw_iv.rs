//! Mark loop induction `±1` updates as NSW-safe for codegen.
//!
//! When a loop header is a strict compare (`<` / `>`) on a mutable slot `iv`,
//! and the body/latch does `iv = iv ± 1`, that add/sub cannot overflow signed
//! i64: the compare prevents the IV from reaching the overflowing extreme.
//! (`<=` / `>=` are intentionally excluded — e.g. `i <= MAX; i = i + 1`.)
//!
//! The `±1` literal may be defined outside the loop body (shared const local);
//! resolution therefore uses a function-wide def map.

use lumia_core::{for_each_block_dfs, Block, Local, Op, Value};
use lumia_syntax::BinOp;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// Locals that are `Binary` Add/Sub results proven safe for NSW emission.
pub(crate) fn collect_nsw_binop_locals(body: &Block) -> HashSet<u32> {
    // Owned facts only (Int / Name / Binary) — enough for IV step recognition.
    let mut all_defs: HashMap<u32, Value> = HashMap::default();
    for_each_block_dfs(body, &mut |b| {
        for op in &b.ops {
            if let Op::Let { local, value, .. } = op {
                if matches!(value, Value::Int(_) | Value::Name(_) | Value::Binary { .. }) {
                    all_defs.insert(local.0, value.clone());
                }
            }
        }
    });

    let mut out = HashSet::default();
    for_each_block_dfs(body, &mut |b| {
        for op in &b.ops {
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
                analyze_loop(header, body, latch, &all_defs, &mut out);
            }
        }
    });
    out
}

fn analyze_loop(
    header: &Block,
    body: &Block,
    latch: &Block,
    all_defs: &HashMap<u32, Value>,
    out: &mut HashSet<u32>,
) {
    let ivs = strict_iv_names(header, all_defs);
    if ivs.is_empty() {
        return;
    }
    for name in &ivs {
        mark_unit_steps(body, name, all_defs, out);
        mark_unit_steps(latch, name, all_defs, out);
    }
}

/// Slot names compared with a strict inequality in the loop header result.
fn strict_iv_names(header: &Block, all_defs: &HashMap<u32, Value>) -> HashSet<String> {
    let mut names = HashSet::default();
    let Some(res) = header.result else {
        return names;
    };
    let Some(Value::Binary {
        op, left, right, ..
    }) = all_defs.get(&res.0)
    else {
        return names;
    };
    if !matches!(op, BinOp::Lt | BinOp::Gt) {
        return names;
    }
    if let Some(n) = name_of_local(*left, all_defs) {
        names.insert(n);
    }
    if let Some(n) = name_of_local(*right, all_defs) {
        names.insert(n);
    }
    names
}

fn mark_unit_steps(
    block: &Block,
    iv: &str,
    all_defs: &HashMap<u32, Value>,
    out: &mut HashSet<u32>,
) {
    for op in &block.ops {
        let Op::Assign {
            name,
            value: Local(dest),
        } = op
        else {
            continue;
        };
        if name != iv {
            continue;
        }
        let Some(Value::Binary {
            op, left, right, ..
        }) = all_defs.get(dest)
        else {
            continue;
        };
        let step = match op {
            BinOp::Add => 1i64,
            BinOp::Sub => -1i64,
            _ => continue,
        };
        let l_iv = name_of_local(*left, all_defs).as_deref() == Some(iv);
        let r_iv = name_of_local(*right, all_defs).as_deref() == Some(iv);
        let l_c = const_i64(*left, all_defs);
        let r_c = const_i64(*right, all_defs);
        let ok = match step {
            1 => (l_iv && r_c == Some(1)) || (r_iv && l_c == Some(1)),
            -1 => l_iv && r_c == Some(1), // iv - 1 only (not 1 - iv)
            _ => false,
        };
        if ok {
            out.insert(*dest);
        }
    }
}

fn name_of_local(l: Local, defs: &HashMap<u32, Value>) -> Option<String> {
    match defs.get(&l.0)? {
        Value::Name(n) => Some(n.clone()),
        _ => None,
    }
}

fn const_i64(l: Local, defs: &HashMap<u32, Value>) -> Option<i64> {
    match defs.get(&l.0)? {
        Value::Int(n) => Some(*n),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
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
    fn skips_le_increment() {
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
        assert!(nsw.is_empty(), "i=i+1 under i<=n must keep overflow checks");
    }

    #[test]
    fn marks_matmul_iv_increments() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/bench_cpu.lm");
        let src = std::fs::read_to_string(&path).unwrap();
        let core =
            lumia_opt::compile_source_to_optimized(&src, &lumia_opt::OptOptions::for_build(true))
                .unwrap();
        let f = core
            .functions
            .iter()
            .find(|f| f.name == "matmulChecksum")
            .unwrap();
        let nsw = collect_nsw_binop_locals(&f.body);
        assert!(
            nsw.len() >= 3,
            "expected i/j/k unit steps under strict <, got {nsw:?}"
        );
    }
}
