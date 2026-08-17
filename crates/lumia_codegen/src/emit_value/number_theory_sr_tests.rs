use super::*;
use lumia_core::collect_loops;
use lumia_opt::{compile_source_to_optimized, OptOptions};

#[test]
fn matches_new_bench_srs() {
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/bench_cpu.lm"
    ))
    .unwrap();
    let core = compile_source_to_optimized(&src, &OptOptions::for_build(true)).unwrap();
    let mut gcd = 0;
    let mut div = 0;
    let mut prod = 0;
    let mut range = 0;
    let mut matmul = 0;
    for f in &core.functions {
        let defs = lumia_core::collect_leaf_defs(&f.body, false);
        let mut loops = vec![];
        collect_loops(&f.body, &mut loops);
        for (h, b, l) in &loops {
            if match_gcd_sum(h, b, l, &defs).is_some() {
                gcd += 1;
            }
            if match_divisor_sum(h, b, l, &defs).is_some() {
                div += 1;
            }
            if match_product_rem_sum(h, b, l, &defs).is_some() {
                prod += 1;
            }
            if match_range_affine1(h, b, l, &defs).is_some() {
                range += 1;
            }
            if match_matmul_affine(h, b, l, &defs).is_some() {
                matmul += 1;
            }
        }
    }
    assert!(gcd >= 1, "gcd matches={gcd}");
    assert!(div >= 1, "div matches={div}");
    assert!(prod >= 1, "prod matches={prod}");
    assert!(range >= 1, "range matches={range}");
    assert!(matmul >= 1, "matmul matches={matmul}");
}
