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
    let mut found_total = 0;
    let mut found_strided = 0;
    for f in &core.functions {
        if !f.name.contains("collatz") && f.name != "main" {
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
            if let Some(p) = match_collatz_total_loop(h, b, l, &defs) {
                assert_eq!(p.limit, 2_500_000);
                assert!(!p.total.is_empty());
                found_total += 1;
            }
            if let Some(p) = match_collatz_strided_loop(h, b, l, &defs) {
                assert_eq!(p.limit, 3_000_000);
                assert_eq!(p.stride, 3);
                found_strided += 1;
            }
        }
    }
    assert!(
        found >= 1,
        "expected at least one collatz loop match, got {found}"
    );
    assert!(
        found_total >= 1,
        "expected at least one collatz-total loop match, got {found_total}"
    );
    assert!(
        found_strided >= 1,
        "expected at least one collatz-strided loop match, got {found_strided}"
    );
}
