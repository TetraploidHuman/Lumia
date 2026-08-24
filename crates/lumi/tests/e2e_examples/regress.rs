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
