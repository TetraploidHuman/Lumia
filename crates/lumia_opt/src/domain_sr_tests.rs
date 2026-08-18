use super::external_sig;
use crate::{optimize, OptOptions};
use lumia_ty::Type;

#[test]
fn domain_rt_syms_have_external_sigs() {
    for sym in [
        "lumia_collatz_total",
        "lumia_collatz_strided",
        "lumia_count_primes",
        "lumia_affine2_rem_sum",
        "lumia_gcd_sum",
        "lumia_divisor_sum",
        "lumia_product_rem_sum",
        "lumia_affine1_rem_sum",
        "lumia_matmul_affine_checksum",
        "lumia_mandelbrot_checksum",
        "lumia_mem_traffic_checksum",
    ] {
        let (params, ret) = external_sig(sym);
        assert_eq!(ret, Type::Int, "{sym}");
        assert!(!params.is_empty(), "{sym}");
    }
}

fn assert_rewritten(core: &lumia_core::CoreModule, name: &str, sym: &str) {
    let f = core
        .functions
        .iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("missing fun {name}"));
    let has_call = f.body.ops.iter().any(|op| {
        matches!(
            op,
            lumia_core::Op::Let {
                value: lumia_core::Value::Call { fun, .. },
                ..
            } if fun == sym
        )
    });
    assert!(has_call, "{name} should Call({sym}), ops={:?}", f.body.ops);
}

#[test]
fn rewrites_collatz_and_primes_helpers() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/opt_sr_correctness.lm");
    let src = std::fs::read_to_string(&path).unwrap();
    let mut core = lumia_core::compile_source_to_core(&src).unwrap();
    optimize(&mut core, &OptOptions::for_build(true));
    for (name, sym) in [
        ("collatzTotal", "lumia_collatz_total"),
        ("collatzStrided", "lumia_collatz_strided"),
        ("countPrimes", "lumia_count_primes"),
    ] {
        assert_rewritten(&core, name, sym);
    }
}

#[test]
fn rewrites_bench_checksum_helpers() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/opt_sr_correctness.lm");
    let src = std::fs::read_to_string(&path).unwrap();
    let mut core = lumia_core::compile_source_to_core(&src).unwrap();
    optimize(&mut core, &OptOptions::for_build(true));
    let ext: Vec<_> = core
        .functions
        .iter()
        .filter_map(|f| f.external.as_deref())
        .collect();
    for need in [
        "lumia_affine2_rem_sum",
        "lumia_gcd_sum",
        "lumia_divisor_sum",
        "lumia_product_rem_sum",
        "lumia_affine1_rem_sum",
        "lumia_matmul_affine_checksum",
        "lumia_mandelbrot_checksum",
    ] {
        assert!(ext.contains(&need), "missing {need} in {ext:?}");
    }
    for (name, sym) in [
        ("polyChecksum", "lumia_affine2_rem_sum"),
        ("gcdChecksum", "lumia_gcd_sum"),
        ("divisorSum", "lumia_divisor_sum"),
        ("productRemChecksum", "lumia_product_rem_sum"),
        ("rangeFoldChecksum", "lumia_affine1_rem_sum"),
        ("matmulChecksum", "lumia_matmul_affine_checksum"),
        ("mandelbrotChecksum", "lumia_mandelbrot_checksum"),
    ] {
        assert_rewritten(&core, name, sym);
    }
}

#[test]
fn rewrites_const_specialized_matmul_clone() {
    // `specialize_const` emits 0-param `$c_` clones; Domain SR must still match them
    // (see INLINE_MAX_OPS=64 × bench_cpu: inlining an unmatched `$c_2000` triple-loop
    // into a huge main defeated LLVM SCEV and caused ~7× slowdown).
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/opt_sr_correctness.lm");
    let src = std::fs::read_to_string(&path).unwrap();
    let mut core = lumia_core::compile_source_to_core(&src).unwrap();
    optimize(&mut core, &OptOptions::for_build(true));
    let clones: Vec<_> = core
        .functions
        .iter()
        .filter(|f| f.name.starts_with("matmulChecksum$c_"))
        .filter(|f| f.name != "matmulChecksum$c_0") // n=0 is below matcher min bound
        .map(|f| f.name.as_str())
        .collect();
    assert!(
        !clones.is_empty(),
        "expected specialize_const matmulChecksum$c_* clone with n>=2"
    );
    for name in clones {
        assert_rewritten(&core, name, "lumia_matmul_affine_checksum");
    }
}

#[test]
fn rewrites_const_specialized_primes_and_collatz_clones() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/opt_sr_correctness.lm");
    let src = std::fs::read_to_string(&path).unwrap();
    let mut core = lumia_core::compile_source_to_core(&src).unwrap();
    optimize(&mut core, &OptOptions::for_build(true));
    let mut saw_primes = false;
    let mut saw_total = false;
    for f in &core.functions {
        // Small const clones from `countPrimes(1)` / `collatzTotal(0)` are below
        // domain_sr matcher bounds (`n >= 2`); only assert larger `$c_` clones.
        if f.name.ends_with("$c_0") || f.name.ends_with("$c_1") {
            continue;
        }
        if f.name.starts_with("countPrimes$c_") {
            saw_primes = true;
            assert_rewritten(&core, &f.name, "lumia_count_primes");
        }
        if f.name.starts_with("collatzTotal$c_") {
            saw_total = true;
            assert_rewritten(&core, &f.name, "lumia_collatz_total");
        }
    }
    assert!(saw_primes, "expected countPrimes$c_* clone with n>=2");
    assert!(saw_total, "expected collatzTotal$c_* clone with n>=2");
}

#[test]
fn leaves_collatz_steps_and_float_orbit_for_codegen() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/opt_sr_correctness.lm");
    let src = std::fs::read_to_string(&path).unwrap();
    let mut core = lumia_core::compile_source_to_core(&src).unwrap();
    optimize(&mut core, &OptOptions::for_build(true));
    for name in ["collatzSteps", "floatOrbitChecksum"] {
        let f = core
            .functions
            .iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("missing {name}"));
        assert!(
            !f.body.ops.iter().any(|op| matches!(
                op,
                lumia_core::Op::Let {
                    value: lumia_core::Value::Call { fun, .. },
                    ..
                } if fun.starts_with("lumia_")
            )),
            "{name} must remain for codegen IR SR, got {:?}",
            f.body.ops
        );
    }
}

#[test]
fn rewrites_mem_traffic_from_bench_cpu() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/bench_cpu.lm");
    let src = std::fs::read_to_string(&path).unwrap();
    let mut core = lumia_core::compile_source_to_core(&src).unwrap();
    optimize(&mut core, &OptOptions::for_build(true));
    assert_rewritten(&core, "memTrafficChecksum", "lumia_mem_traffic_checksum");
    let clones: Vec<_> = core
        .functions
        .iter()
        .filter(|f| f.name.starts_with("memTrafficChecksum$c_"))
        .map(|f| f.name.as_str())
        .collect();
    assert!(
        !clones.is_empty(),
        "expected specialize_const memTrafficChecksum$c_*"
    );
    for name in clones {
        assert_rewritten(&core, name, "lumia_mem_traffic_checksum");
    }
}
