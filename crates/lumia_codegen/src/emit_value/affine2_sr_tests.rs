use super::*;
use lumia_core::collect_loops;
use lumia_opt::{compile_source_to_optimized, OptOptions};

#[test]
fn matches_poly_checksum() {
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/bench_cpu.lm"
    ))
    .unwrap();
    let core = compile_source_to_optimized(&src, &OptOptions::for_build(true)).unwrap();
    let mut found = 0;
    for f in &core.functions {
        if !f.name.contains("poly") && f.name != "main" {
            continue;
        }
        let defs = lumia_core::collect_leaf_defs(&f.body, false);
        let mut loops = vec![];
        collect_loops(&f.body, &mut loops);
        for (h, b, l) in &loops {
            if let Some(p) = match_affine2_rem_sum(h, b, l, &defs) {
                assert_eq!(p.n, 12_000);
                assert_eq!((p.a, p.b, p.c, p.m), (131, 17, 1, 10007));
                found += 1;
            }
        }
    }
    assert!(found >= 1, "expected poly affine2 match, got {found}");
}
