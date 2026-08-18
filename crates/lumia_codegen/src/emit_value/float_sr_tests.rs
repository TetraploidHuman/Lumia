use super::*;
use lumia_core::collect_loops;
use lumia_opt::{compile_source_to_optimized, OptOptions};

#[test]
fn matches_float_orbit_in_bench() {
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/bench_cpu.lm"
    ))
    .unwrap();
    let core = compile_source_to_optimized(&src, &OptOptions::for_build(true)).unwrap();
    let mut fo = 0;
    for f in &core.functions {
        let defs = lumia_core::collect_leaf_defs(&f.body, false);
        let mut loops = vec![];
        collect_loops(&f.body, &mut loops);
        for (h, b, l) in &loops {
            if match_float_orbit(h, b, l, &defs).is_some() {
                fo += 1;
            }
        }
    }
    assert!(fo >= 1, "floatOrbit matches={fo}");
    assert!(
        core.functions
            .iter()
            .any(|f| f.external.as_deref() == Some("lumia_mandelbrot_checksum")),
        "opt domain_sr should rewrite mandelbrotChecksum"
    );
}

#[test]
fn matches_float_orbit_in_opt_sr_correctness() {
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/opt_sr_correctness.lm"
    ))
    .unwrap();
    let core = compile_source_to_optimized(&src, &OptOptions::for_build(true)).unwrap();
    let mut fo = 0;
    for f in &core.functions {
        let defs = lumia_core::collect_leaf_defs(&f.body, false);
        let mut loops = vec![];
        collect_loops(&f.body, &mut loops);
        for (h, b, l) in &loops {
            if match_float_orbit(h, b, l, &defs).is_some() {
                fo += 1;
            }
        }
    }
    assert!(fo >= 1, "floatOrbit matches={fo}");
}
