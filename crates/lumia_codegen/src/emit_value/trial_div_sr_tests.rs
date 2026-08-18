use super::*;
use lumia_core::collect_loops;
use lumia_opt::{compile_source_to_optimized, OptOptions};

#[test]
fn matches_is_prime_trial_loop() {
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/bench_cpu.lm"
    ))
    .unwrap();
    let core = compile_source_to_optimized(&src, &OptOptions::for_build(true)).unwrap();
    let mut found = 0;
    for f in &core.functions {
        if !f.name.contains("Prime") && !f.name.contains("prime") {
            continue;
        }
        let defs = lumia_core::collect_leaf_defs(&f.body, false);
        let mut loops = vec![];
        collect_loops(&f.body, &mut loops);
        for (h, b, l) in &loops {
            if match_trial_div_loop(h, b, l, &defs).is_some() {
                found += 1;
            }
        }
    }
    assert!(found >= 1, "expected trial-div match, got {found}");
    // Whole-fn countPrimes lives in lumia_opt::domain_sr now.
    assert!(
        core.functions
            .iter()
            .any(|f| f.external.as_deref() == Some("lumia_count_primes")),
        "opt domain_sr should rewrite countPrimes"
    );
}
