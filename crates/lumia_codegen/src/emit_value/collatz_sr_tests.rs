use super::*;
use lumia_core::collect_loops;
use lumia_opt::{compile_source_to_optimized, OptOptions};

#[test]
fn matches_collatz_steps_loop() {
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/bench_cpu.lm"
    ))
    .unwrap();
    let core = compile_source_to_optimized(&src, &OptOptions::for_build(true)).unwrap();
    let mut found = 0;
    for f in &core.functions {
        if f.name != "collatzSteps" && !f.name.starts_with("collatzSteps$") {
            continue;
        }
        let defs = lumia_core::collect_leaf_defs(&f.body, false);
        let mut loops = vec![];
        collect_loops(&f.body, &mut loops);
        for (h, b, l) in &loops {
            if let Some(p) = match_collatz_loop(h, b, l, &defs) {
                assert!(!p.x.is_empty() && !p.steps.is_empty());
                found += 1;
            }
        }
    }
    assert!(
        found >= 1,
        "expected at least one collatz steps loop match, got {found}"
    );
    // Whole-fn total/strided live in lumia_opt::domain_sr now.
    assert!(
        core.functions
            .iter()
            .any(|f| f.external.as_deref() == Some("lumia_collatz_total")),
        "opt domain_sr should rewrite collatzTotal"
    );
}
