#[test]
fn e2e_ffi_abs() {
    run_example_trust_foreign_pure("examples/guide/ffi_abs.lm", &["42", "7"]);
}

#[test]
fn e2e_ffi_strlen() {
    run_example_trust_foreign_pure("examples/guide/ffi_strlen.lm", &["5", "0"]);
}

e2e_ok!(e2e_ffi_getenv, "examples/guide/ffi_getenv.lm", "true", "0");

e2e_ok!(e2e_use_path_dep, "examples/guide/use_path_dep.lm", "42", "42");
