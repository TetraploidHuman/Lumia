e2e_ok!(
    e2e_show_float_adt,
    "examples/show_float_adt.lm",
    "Vec2(1.5, 2.25)"
);

#[test]
fn e2e_float_ops() {
    run_example("examples/float_ops.lm", &["3.75", "6", "1", "-1.5", "4"]);
}

#[test]
fn e2e_float_map_keys() {
    // ±0 collide; NaN never hits contains (matches IEEE ==).
    run_example(
        "examples/float_map_keys.lm",
        &["true", "1", "false", "0", "true", "true"],
    );
}

#[test]
fn e2e_float_struct_eq() {
    // List/Option/Map Float payloads + ListParMap: ±0 equal; NaN ≠ NaN.
    run_example(
        "examples/float_struct_eq.lm",
        &["1", "0", "1", "0", "1", "0", "0"],
    );
}

#[test]
fn e2e_adt_float_eq() {
    // Sum arity-safe IEEE eq; mono wrappers keep Result/Option ret (not Float bits).
    run_example(
        "examples/adt_float_eq.lm",
        &["1", "0", "1", "0", "1", "0", "1", "0"],
    );
}

#[test]
fn e2e_nested_float_adt_eq() {
    // Nested Option[Float] in List/Set: layout mask on ADT header → IEEE via lumia_eq.
    run_example(
        "examples/nested_float_adt_eq.lm",
        &["1", "0", "1", "1", "1"],
    );
}

#[test]
fn e2e_dense_float_kernels() {
    // gemv [5,11,17]; gemvT [4,6]; addmm sum 21; L2 [0.6,0.8]; 16×32 nucleus gemv
    run_example(
        "examples/dense_float_kernels.lm",
        &["33000", "10000", "21000", "1400", "2261"],
    );
}

