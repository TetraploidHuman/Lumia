// CLI `--no-inline` / pass flags smoke (stock semantics).

#[test]
fn build_no_inline_poly_map_id() {
    run_example_build(
        "examples/poly_map_id.lm",
        None,
        &["2", "true", "2", "true"],
        false,
        &["--no-inline"],
    );
}

#[test]
fn build_no_dense_f64_float_map() {
    run_example_build(
        "examples/float_map_overlay_keys.lm",
        None,
        &["200", "200", "10"],
        false,
        &["--no-dense-f64"],
    );
}
