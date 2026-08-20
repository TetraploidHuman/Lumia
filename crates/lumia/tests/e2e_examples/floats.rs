e2e_ok!(
    e2e_show_float_adt,
    "examples/guide/show_float_adt.lm",
    "Vec2(1.5, 2.25)"
);

e2e_ok!(
    e2e_float_list_map,
    "examples/guide/float_list_map.lm",
    "3",
    "true",
    "true",
    "true"
);

#[test]
fn e2e_float_ops() {
    run_example("examples/guide/float_ops.lm", &["3.75", "6", "1", "-1.5", "4"]);
}

#[test]
fn e2e_float_map_keys() {
    // ±0 collide; NaN never hits contains (matches IEEE ==).
    run_example(
        "examples/guide/float_map_keys.lm",
        &["true", "1", "false", "0", "true", "true"],
    );
}

#[test]
fn e2e_float_map_overlay_keys() {
    // Overlay on HashOrdered Float map: ±0 same key; last write wins.
    run_example("examples/guide/float_map_overlay_keys.lm", &["200", "200", "10"]);
}

#[test]
fn e2e_var_scoped_gc() {
    // Heap `var` first rooted inside `for` must survive GC across iterations.
    // sum_i (1+i)+2 for i in 0..50 = sum (3+i) = 50*3 + 49*50/2 = 150+1225 = 1375
    run_example("examples/guide/var_scoped_gc.lm", &["1375"]);
}

#[test]
fn e2e_cow_nested_list() {
    run_example("examples/guide/cow_nested_list.lm", &["2", "1", "2", "3"]);
}

#[test]
fn e2e_empty_float_list() {
    run_example("examples/guide/empty_float_list.lm", &["1", "0"]);
}

#[test]
fn e2e_float_struct_eq() {
    // List/Option/Map Float payloads + ListParMap: ±0 equal; NaN ≠ NaN.
    run_example(
        "examples/guide/float_struct_eq.lm",
        &["1", "0", "1", "0", "1", "0", "0"],
    );
}

#[test]
fn e2e_adt_float_eq() {
    // Sum arity-safe IEEE eq; mono wrappers keep Result/Option ret (not Float bits).
    run_example(
        "examples/guide/adt_float_eq.lm",
        &["1", "0", "1", "0", "1", "0", "1", "0"],
    );
}

#[test]
fn e2e_nested_float_adt_eq() {
    // Nested Option[Float] in List/Set: layout mask on ADT header → IEEE via lumia_eq.
    run_example(
        "examples/guide/nested_float_adt_eq.lm",
        &["1", "0", "1", "1", "1"],
    );
}

#[test]
fn e2e_result_float_err_eq() {
    // Result[Float, String] typed == must use per-object `_pad`, not type-param
    // float mask (Err field0 is params[1], not Float).
    run_example(
        "examples/guide/result_float_err_eq.lm",
        &["1", "0", "1", "1", "0"],
    );
}

#[test]
fn e2e_wide_float_adt_gc() {
    // Float past field index 31 must stay in the u64 ADT mask (GC must not follow bits).
    run_example("examples/guide/wide_float_adt_gc.lm", &["1.25", "2.5"]);
}

#[test]
fn e2e_mono_adt_float_field() {
    // Call-site ABI Int must not make mono clones sitofp product float fields.
    run_example("examples/guide/mono_adt_float_field.lm", &["3"]);
}

#[test]
fn e2e_var_adt_float_mut() {
    // `var s = adt.floatField` must bitcast IEEE bits into an f64 mut slot (not sitofp).
    run_example(
        "examples/guide/var_adt_float_mut.lm",
        &["0.48052464447939985", "0.48052464447939985"],
    );
}

#[test]
fn e2e_poly_list_float_get() {
    // Poly `{ pts -> pts.get(0) }` called with `var` List[Float] must not println
    // IEEE bit patterns.
    run_example(
        "examples/guide/poly_list_float_get.lm",
        &["0.668", "0.668", "1.1280000000000001"],
    );
}

#[test]
fn e2e_adt_field_call_arg() {
    // `nearest(eco, eco.ecoThreats, n)` must keep the field list live across the call.
    run_example(
        "examples/guide/adt_field_call_arg.lm",
        &["0.668", "0.46", "0.535372767331324"],
    );
}

#[test]
fn e2e_dense_float_kernels() {
    // gemv [5,11,17]; gemvT [4,6]; addmm sum 21; L2 [0.6,0.8]; 16×32 nucleus gemv
    run_example(
        "examples/guide/dense_float_kernels.lm",
        &["33000", "10000", "21000", "1400", "2261"],
    );
}

