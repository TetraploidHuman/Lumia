e2e_ok!(e2e_poly_id, "examples/poly_id.lm", "2", "1.5", "hi", "3.5");

#[test]
fn e2e_poly_inc() {
    // Float monomorphization of `{ x -> x + x }` (not just identity).
    run_example("examples/poly_inc.lm", &["2", "3"]);
}

e2e_ok!(e2e_poly_add1, "examples/poly_add1.lm", "2", "2.5");

#[test]
fn e2e_poly_top_dbl() {
    // Top-level `val dbl` Float site → `dbl$Float` clone.
    run_example("examples/poly_top_dbl.lm", &["2", "3"]);
}

e2e_ok!(e2e_poly_bool, "examples/poly_bool.lm", "1", "true", "false");

#[test]
fn e2e_poly_str() {
    // String sites → `$String` clone; Bool site → `$Bool`; Int shared body.
    run_example("examples/poly_str.lm", &["[hi]", "ok", "42", "true"]);
}

e2e_ok!(e2e_poly_option, "examples/poly_option.lm", "7", "1.5");

e2e_ok!(e2e_poly_list, "examples/poly_list.lm", "20", "2.5");

#[test]
fn e2e_poly_unwrap() {
    run_example("examples/poly_unwrap.lm", &["7", "-1", "hi", "no"]);
}

#[test]
fn e2e_poly_map_id() {
    run_example("examples/poly_map_id.lm", &["2", "true", "2", "true"]);
}

#[test]
fn e2e_poly_set_id() {
    run_example("examples/poly_set_id.lm", &["3", "true", "2", "true"]);
}

#[test]
fn e2e_poly_option_map() {
    // FunRef HOF mono: Option map at Int / Float / String.
    run_example("examples/poly_option_map.lm", &["42", "3", "-1", "hi!"]);
}

#[test]
fn e2e_poly_option_and_then() {
    run_example("examples/poly_option_and_then.lm", &["5", "-1", "-2"]);
}

#[test]
fn e2e_poly_result_map() {
    run_example("examples/poly_result_map.lm", &["42", "3", "boom"]);
}

#[test]
fn e2e_small_list_local() {
    // Non-escaping small listOf → stack LitList; len/get still work.
    run_example("examples/small_list_local.lm", &["3", "10", "30", "60"]);
}

#[test]
fn e2e_small_map_local() {
    run_example("examples/small_map_local.lm", &["3", "true", "10", "30"]);
}

e2e_ok!(
    e2e_small_set_local,
    "examples/small_set_local.lm",
    "3",
    "true",
    "false"
);

#[test]
fn e2e_pe_list_len_get() {
    // Same output as small_list_local; ListLen/ListGet folded at opt L0 when possible.
    run_example("examples/pe_list_len_get.lm", &["3", "10", "30", "60"]);
}

e2e_ok!(
    e2e_pe_adt_field,
    "examples/pe_adt_field.lm",
    "10",
    "20",
    "30"
);

#[test]
fn e2e_pe_map_contains() {
    // Const-fold mapOf/setOf → len / contains (memo L0).
    run_example(
        "examples/pe_map_contains.lm",
        &["3", "true", "false", "3", "true", "false"],
    );
}

#[test]
fn e2e_escape_pure_len() {
    // Pure len callee must not force list escape → LitList still works.
    run_example("examples/escape_pure_len.lm", &["3", "20"]);
}

#[test]
fn e2e_small_adt_local() {
    // Non-escaping product via non-capturing field getters → LitAdt.
    run_example("examples/small_adt_local.lm", &["10", "20", "30"]);
}

e2e_ok!(
    e2e_hof_float_to_int,
    "examples/hof_float_to_int.lm",
    "1",
    "2"
);

#[test]
fn e2e_hof_float_apply() {
    // HOF mono clone must keep Float return ABI after directizing to dbl$Float.
    run_example("examples/hof_float_apply.lm", &["3", "3", "4"]);
}
