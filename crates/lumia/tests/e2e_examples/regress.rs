// Former `examples/_bugs/` cases — fixed regressions kept as e2e goldens.

#[test]
fn e2e_regress_field_list_arg_gc() {
    run_example("examples/regress/field_list_arg_gc.lm", &["12", "12"]);
}

#[test]
fn e2e_regress_list_get_int() {
    run_example(
        "examples/regress/list_get_int.lm",
        &["0.668", "0.668", "1.1280000000000001", "1.1280000000000001"],
    );
}

#[test]
fn e2e_regress_makeobs_in() {
    run_example(
        "examples/regress/makeobs_in.lm",
        &["0.668", "0.46", "0.535372767331324"],
    );
}

#[test]
fn e2e_regress_makeobs_let() {
    run_example(
        "examples/regress/makeobs_let.lm",
        &["0.668", "0.668", "0.668"],
    );
}

#[test]
fn e2e_regress_makeobs_let2() {
    run_example(
        "examples/regress/makeobs_let2.lm",
        &[
            "0.668",
            "0.668",
            "0.668",
            "1.1280000000000001",
            "0.668",
            "1.1280000000000001",
            "0.668",
            "1.1280000000000001",
        ],
    );
}

#[test]
fn e2e_regress_makeobs_nofill() {
    run_example(
        "examples/regress/makeobs_nofill.lm",
        &["0.668", "0.46", "0.535372767331324"],
    );
}

#[test]
fn e2e_regress_makeobs_threat() {
    run_example(
        "examples/regress/makeobs_threat.lm",
        &["0.668", "0.46", "0.668", "0.46"],
    );
}

#[test]
fn e2e_regress_makeobs_threat2() {
    run_example("examples/regress/makeobs_threat2.lm", &["0.668", "0.46"]);
}

#[test]
fn e2e_regress_mono_list() {
    run_example("examples/regress/mono_list.lm", &["1.25", "1.25"]);
}

#[test]
fn e2e_regress_mono_list2() {
    run_example("examples/regress/mono_list2.lm", &["1.25", "3.75"]);
}

#[test]
fn e2e_regress_nearest_elision() {
    run_example(
        "examples/regress/nearest_elision.lm",
        &["2.578175583005965", "2.578175583005965"],
    );
}

#[test]
fn e2e_regress_println_float() {
    // Whole-number floats print without trailing `.0` (Rust `Display` for f64).
    run_example(
        "examples/regress/println_float.lm",
        &[
            "2106498180",
            "0.4805246444793999",
            "3.5",
            "2106498180",
            "2106498180",
        ],
    );
}

#[test]
fn e2e_regress_println_adt_float() {
    run_example(
        "examples/regress/println_adt_float.lm",
        &[
            "0.20000000307336452",
            "0.20014835745103116",
            "-0.14010811325053796",
            "0.43020336673604487",
            "2106498180",
        ],
    );
}

#[test]
fn e2e_regress_spawn_id_fun_float() {
    run_example("examples/regress/spawn_id_fun_float.lm", &["1.5", "2.5"]);
}

#[test]
fn e2e_regress_spawn_string_cap_len() {
    run_example("examples/regress/spawn_string_cap_len.lm", &["prex", "4"]);
}

#[test]
fn e2e_regress_andthen_unwrapor_float() {
    run_example("examples/regress/andthen_unwrapor_float.lm", &["3"]);
}

#[test]
fn e2e_regress_spawn_option_map_unwrapor() {
    run_example("examples/regress/spawn_option_map_unwrapor.lm", &["4.5"]);
}

#[test]
fn e2e_regress_unwrapor_fun_float() {
    run_example("examples/regress/unwrapor_fun_float.lm", &["2.5"]);
}

#[test]
fn e2e_regress_spawn_bool_option() {
    run_example(
        "examples/regress/spawn_bool_option.lm",
        &["Some(true)", "[true, false]", "true", "1.5"],
    );
}

#[test]
fn e2e_regress_spawn_foldsum_float() {
    run_example("examples/regress/spawn_foldsum_float.lm", &["9"]);
}

#[test]
fn e2e_regress_nested_andthen_unwrapor() {
    run_example("examples/regress/nested_andthen_unwrapor.lm", &["4"]);
}

#[test]
fn e2e_regress_nested_option_unwrapor_int() {
    run_example(
        "examples/regress/nested_option_unwrapor_int.lm",
        &["3", "3", "3", "3"],
    );
}

#[test]
fn e2e_regress_nested_result_unwrapor_int() {
    run_example("examples/regress/nested_result_unwrapor_int.lm", &["3"]);
}

#[test]
fn e2e_regress_ufcs_open_recv() {
    run_example(
        "examples/regress/ufcs_open_recv.lm",
        &["10", "9", "99", "true", "true", "2", "1"],
    );
}

#[test]
fn e2e_regress_set_map_literal_dedup() {
    run_example(
        "examples/regress/set_map_literal_dedup.lm",
        &["2", "1", "2", "1", "20", "1"],
    );
}

#[test]
fn e2e_regress_either_mixed_payload() {
    run_example("examples/regress/either_mixed_payload.lm", &["2", "3"]);
}

#[test]
fn e2e_regress_nat_to_int() {
    run_example("examples/regress/nat_to_int.lm", &["0", "1", "3"]);
}

#[test]
fn e2e_regress_ulist_sum() {
    run_example("examples/regress/ulist_sum.lm", &["0", "6"]);
}

#[test]
fn e2e_regress_expr_eval() {
    run_example("examples/regress/expr_eval.lm", &["7", "5", "7"]);
}

#[test]
fn e2e_regress_nested_result_andthen_unwrapor() {
    run_example("examples/regress/nested_result_andthen_unwrapor.lm", &["3"]);
}

#[test]
fn e2e_regress_option_map_id_int() {
    run_example("examples/regress/option_map_id_int.lm", &["42"]);
}

#[test]
fn e2e_regress_result_map_id_int() {
    run_example("examples/regress/result_map_id_int.lm", &["7"]);
}

#[test]
fn e2e_regress_unwrapor_none_defaults() {
    run_example(
        "examples/regress/unwrapor_none_defaults.lm",
        &["1.5", "true"],
    );
}

#[test]
fn e2e_regress_unwrapor_err_float() {
    run_example("examples/regress/unwrapor_err_float.lm", &["1.5"]);
}

#[test]
fn e2e_regress_spawn_list_map_id_fun() {
    run_example(
        "examples/regress/spawn_list_map_id_fun.lm",
        &["2.5", "3.5"],
    );
}

#[test]
fn e2e_regress_flatmap_list_fun() {
    run_example("examples/regress/flatmap_list_fun.lm", &["2"]);
}

#[test]
fn e2e_regress_box_fun_spawn() {
    run_example("examples/regress/box_fun_spawn.lm", &["2.5"]);
}

#[test]
fn e2e_regress_channel_named_fun() {
    run_example("examples/regress/channel_named_fun.lm", &["3"]);
}

#[test]
fn e2e_regress_option_id_fun() {
    run_example("examples/regress/option_id_fun.lm", &["2.5"]);
}

#[test]
fn e2e_regress_ok_id_fun() {
    run_example("examples/regress/ok_id_fun.lm", &["2.5"]);
}

#[test]
fn e2e_regress_list_curried_fun() {
    run_example("examples/regress/list_curried_fun.lm", &["4"]);
}

#[test]
fn e2e_regress_fold_cap_list_float() {
    run_example("examples/regress/fold_cap_list_float.lm", &["3"]);
}

#[test]
fn e2e_regress_fold_hof_param_float() {
    run_example(
        "examples/regress/fold_hof_param_float.lm",
        &["6", "6", "6", "3", "3", "6", "6"],
    );
}

#[test]
fn e2e_regress_string_poly_len_concat() {
    run_example(
        "examples/regress/string_poly_len_concat.lm",
        &["2", "2", "3", "Some(2)", "abc"],
    );
}

#[test]
fn e2e_regress_string_utf8_len() {
    run_example(
        "examples/regress/string_utf8_len.lm",
        &["2", "你", "你好", "好", "3", "😀"],
    );
}

#[test]
fn e2e_regress_string_take_reverse() {
    run_example(
        "examples/regress/string_take_reverse.lm",
        &["你好", "世界", "好ba", "2"],
    );
}

#[test]
fn e2e_regress_string_open_take_case() {
    run_example(
        "examples/regress/string_open_take_case.lm",
        &["你好", "好ba", "äbc", "CAFÉ"],
    );
}

#[test]
fn e2e_regress_num_vec2_float() {
    run_example("examples/regress/num_vec2_float.lm", &["2", "3"]);
}

#[test]
fn e2e_regress_float_pm0_map_set() {
    run_example(
        "examples/regress/float_pm0_map_set.lm",
        &["1", "2", "2", "1", "true", "true"],
    );
}

#[test]
fn e2e_regress_tco_list_bool_get() {
    run_example("examples/regress/tco_list_bool_get.lm", &["true", "false"]);
}

#[test]
fn e2e_regress_tco_list_float_get() {
    run_example("examples/regress/tco_list_float_get.lm", &["6"]);
}

#[test]
fn e2e_regress_channel_option_result() {
    run_example(
        "examples/regress/channel_option_result.lm",
        &["1.5", "-1", "Ok(1.5)", "e"],
    );
}

#[test]
fn e2e_regress_prelude_ctor_first_class() {
    run_example(
        "examples/regress/prelude_ctor_first_class.lm",
        &["1.5", "2", "true"],
    );
}
